use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{
    AppError, Result,
    downloads::{DownloadJob, DownloadLog, InstalledItem, timestamp},
    settings::Settings,
    workshop::{DownloadBatch, WorkshopPreview, external::ExternalItem},
};

pub struct Storage(Mutex<Connection>);

impl Storage {
    pub fn open(path: &Path, defaults: &Settings) -> Result<Self> {
        Self::initialize(Connection::open(path)?, defaults)
    }

    fn initialize(mut connection: Connection, defaults: &Settings) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(3))?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: u32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => tx.execute_batch(
                "CREATE TABLE settings (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    payload TEXT NOT NULL
                );
                CREATE TABLE download_jobs (
                    id TEXT PRIMARY KEY NOT NULL,
                    payload TEXT NOT NULL
                );
                PRAGMA user_version = 1;",
            )?,
            1..=3 => {}
            _ => {
                return Err(AppError::Invalid(
                    "The application database was created by a newer version.".into(),
                ));
            }
        }
        if version < 2 {
            tx.execute_batch(
                "ALTER TABLE download_jobs ADD COLUMN queue_order INTEGER NOT NULL DEFAULT 0;
                 UPDATE download_jobs SET queue_order = rowid;
                 UPDATE download_jobs SET payload = json_set(payload, '$.status', 'downloading')
                    WHERE json_extract(payload, '$.status') = 'running';
                 CREATE INDEX download_queue_order ON download_jobs(queue_order)
                    WHERE json_extract(payload, '$.status') = 'queued';
                 CREATE TABLE installed_items (
                    app_id INTEGER NOT NULL,
                    item_id TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    PRIMARY KEY (app_id, item_id)
                 );
                 CREATE TABLE download_logs (
                    id INTEGER PRIMARY KEY,
                    job_id TEXT NOT NULL REFERENCES download_jobs(id) ON DELETE CASCADE,
                    time TEXT NOT NULL,
                    message TEXT NOT NULL
                 );
                 CREATE INDEX download_logs_by_job ON download_logs(job_id, id);
                 PRAGMA user_version = 2;",
            )?;
        }
        if version < 3 {
            tx.execute_batch(
                "CREATE TABLE workshop_previews (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
                 CREATE TABLE collections (item_id TEXT PRIMARY KEY, payload TEXT NOT NULL);
                 CREATE TABLE collection_members (collection_id TEXT NOT NULL REFERENCES collections(item_id), item_id TEXT NOT NULL, PRIMARY KEY(collection_id,item_id));
                 CREATE TABLE download_batches (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
                 CREATE TABLE batch_jobs (batch_id TEXT NOT NULL REFERENCES download_batches(id), item_id TEXT NOT NULL, job_id TEXT REFERENCES download_jobs(id), PRIMARY KEY(batch_id,item_id));
                 CREATE TABLE external_items (app_id INTEGER NOT NULL, item_id TEXT NOT NULL, path TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(app_id,item_id,path));
                 PRAGMA user_version = 3;"
            )?;
        }
        let exists = tx
            .query_row("SELECT 1 FROM settings WHERE singleton = 1", [], |_| Ok(()))
            .optional()?
            .is_some();
        if !exists {
            defaults.validate()?;
            tx.execute(
                "INSERT INTO settings VALUES (1, ?1)",
                [serde_json::to_string(defaults)?],
            )?;
        }
        tx.commit()?;
        connection.pragma_update(None, "foreign_keys", true)?;
        Ok(Self(Mutex::new(connection)))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.0
            .lock()
            .map_err(|_| AppError::Invalid("The application database is unavailable.".into()))
    }

    pub fn settings(&self) -> Result<Settings> {
        let value: String = self.lock()?.query_row(
            "SELECT payload FROM settings WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        Ok(serde_json::from_str(&value)?)
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        settings.validate()?;
        self.lock()?.execute(
            "UPDATE settings SET payload = ?1 WHERE singleton = 1",
            [serde_json::to_string(settings)?],
        )?;
        Ok(())
    }

    pub fn put_job(&self, job: &DownloadJob) -> Result<()> {
        self.lock()?.execute(
            "INSERT INTO download_jobs (id, payload, queue_order)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(queue_order), 0) + 1 FROM download_jobs))
             ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
            params![job.id, serde_json::to_string(job)?],
        )?;
        Ok(())
    }

    pub fn requeue_job(&self, job: &DownloadJob) -> Result<()> {
        self.lock()?.execute(
            "UPDATE download_jobs SET payload = ?2, queue_order =
               (SELECT COALESCE(MAX(queue_order), 0) + 1 FROM download_jobs) WHERE id = ?1",
            params![job.id, serde_json::to_string(job)?],
        )?;
        Ok(())
    }

    pub fn jobs(&self) -> Result<Vec<DownloadJob>> {
        self.read_payloads("SELECT payload FROM download_jobs ORDER BY rowid DESC")
    }

    pub fn job(&self, id: &str) -> Result<Option<DownloadJob>> {
        self.read_payload("SELECT payload FROM download_jobs WHERE id = ?1", [id])
    }

    pub fn queued_jobs(&self) -> Result<Vec<DownloadJob>> {
        self.read_payloads(
            "SELECT payload FROM download_jobs WHERE json_extract(payload, '$.status') = 'queued'
             ORDER BY queue_order",
        )
    }

    pub fn remove_job(&self, id: &str) -> Result<()> {
        let connection = self.lock()?;
        let member: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM batch_jobs WHERE job_id=?1)",
            [id],
            |row| row.get(0),
        )?;
        if member {
            return Err(AppError::Invalid(
                "This job belongs to a batch. Its history and logs are preserved with that batch."
                    .into(),
            ));
        }
        connection.execute("DELETE FROM download_jobs WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn installed(&self) -> Result<Vec<InstalledItem>> {
        self.read_payloads("SELECT payload FROM installed_items")
    }

    pub fn installed_by_id(&self, item_id: &str) -> Result<Option<InstalledItem>> {
        self.read_payload(
            "SELECT payload FROM installed_items WHERE item_id = ?1 LIMIT 1",
            [item_id],
        )
    }

    pub fn installed_item(&self, app_id: u32, item_id: &str) -> Result<Option<InstalledItem>> {
        self.read_payload(
            "SELECT payload FROM installed_items WHERE app_id = ?1 AND item_id = ?2",
            params![app_id, item_id],
        )
    }

    pub fn complete_job(&self, job: &DownloadJob, item: &InstalledItem) -> Result<()> {
        let mut connection = self.lock()?;
        let tx = connection.transaction()?;
        tx.execute(
            "UPDATE download_jobs SET payload = ?2 WHERE id = ?1",
            params![job.id, serde_json::to_string(job)?],
        )?;
        tx.execute(
            "INSERT INTO installed_items (app_id, item_id, payload) VALUES (?1, ?2, ?3)
             ON CONFLICT(app_id, item_id) DO UPDATE SET payload = excluded.payload",
            params![item.app_id, item.item_id, serde_json::to_string(item)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn log(&self, job_id: &str, message: &str) -> Result<()> {
        let mut connection = self.lock()?;
        let tx = connection.transaction()?;
        tx.execute(
            "INSERT INTO download_logs (job_id, time, message) VALUES (?1, ?2, ?3)",
            params![job_id, timestamp(), message],
        )?;
        tx.execute(
            "DELETE FROM download_logs WHERE job_id = ?1 AND id <=
               (SELECT id FROM download_logs WHERE job_id = ?1 ORDER BY id DESC LIMIT 1 OFFSET 200)",
            [job_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn logs(&self, id: &str) -> Result<Vec<DownloadLog>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT time, message FROM download_logs WHERE job_id = ?1 ORDER BY id")?;
        let rows = statement.query_map([id], |row| {
            Ok(DownloadLog {
                time: row.get(0)?,
                message: row.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn save_preview(&self, preview: &WorkshopPreview) -> Result<()> {
        let mut connection = self.lock()?;
        let tx = connection.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO workshop_previews VALUES (?1,?2)",
            params![preview.id, serde_json::to_string(preview)?],
        )?;
        // Keep only the 100 most recent selection previews; collection metadata has its own record.
        tx.execute("DELETE FROM workshop_previews WHERE rowid NOT IN (SELECT rowid FROM workshop_previews ORDER BY rowid DESC LIMIT 100)",[])?;
        if let Some(id) = &preview.collection_id {
            tx.execute(
                "INSERT INTO collections VALUES (?1,?2) ON CONFLICT(item_id) DO UPDATE SET payload=excluded.payload",
                params![id, serde_json::to_string(preview)?],
            )?;
            tx.execute(
                "DELETE FROM collection_members WHERE collection_id=?1",
                [id],
            )?;
            for item in &preview.items {
                tx.execute(
                    "INSERT INTO collection_members VALUES (?1,?2)",
                    params![id, item.id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub fn preview(&self, id: &str) -> Result<WorkshopPreview> {
        self.read_payload("SELECT payload FROM workshop_previews WHERE id=?1", [id])?
            .ok_or_else(|| {
                AppError::Invalid("Preview expired. Open the collection or selection again.".into())
            })
    }
    pub fn put_batch(&self, batch: &DownloadBatch, jobs: &[DownloadJob]) -> Result<()> {
        let mut connection = self.lock()?;
        let tx = connection.transaction()?;
        tx.execute(
            "INSERT INTO download_batches VALUES (?1,?2)",
            params![batch.id, serde_json::to_string(batch)?],
        )?;
        for job in jobs {
            tx.execute("INSERT INTO download_jobs (id,payload,queue_order) VALUES (?1,?2,(SELECT COALESCE(MAX(queue_order),0)+1 FROM download_jobs))",params![job.id,serde_json::to_string(job)?])?;
        }
        for entry in &batch.items {
            tx.execute(
                "INSERT INTO batch_jobs VALUES (?1,?2,?3)",
                params![batch.id, entry.item_id, entry.job_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn batches(&self) -> Result<Vec<DownloadBatch>> {
        self.read_payloads("SELECT payload FROM download_batches ORDER BY rowid DESC")
    }
    pub fn batch(&self, id: &str) -> Result<DownloadBatch> {
        self.read_payload("SELECT payload FROM download_batches WHERE id=?1", [id])?
            .ok_or_else(|| AppError::Invalid("Batch not found.".into()))
    }
    pub fn external_items(&self) -> Result<Vec<ExternalItem>> {
        self.read_payloads("SELECT payload FROM external_items ORDER BY rowid DESC")
    }
    pub fn save_external(&self, item: &ExternalItem) -> Result<bool> {
        let connection = self.lock()?;
        let exists:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM external_items WHERE app_id=?1 AND item_id=?2 AND path=?3)",params![item.item.app_id,item.item.id,item.path],|row|row.get(0))?;
        connection.execute("INSERT INTO external_items VALUES (?1,?2,?3,?4) ON CONFLICT(app_id,item_id,path) DO UPDATE SET payload=excluded.payload",params![item.item.app_id,item.item.id,item.path,serde_json::to_string(item)?])?;
        Ok(!exists)
    }

    fn read_payload<T: serde::de::DeserializeOwned>(
        &self,
        query: &str,
        args: impl rusqlite::Params,
    ) -> Result<Option<T>> {
        let result: Option<String> = self
            .lock()?
            .query_row(query, args, |row| row.get(0))
            .optional()?;
        result
            .map(|payload| Ok(serde_json::from_str(&payload)?))
            .transpose()
    }

    fn read_payloads<T: serde::de::DeserializeOwned>(&self, query: &str) -> Result<Vec<T>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(query)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        downloads::{DownloadStatus, QueueOptions},
        settings::{FolderNaming, test_root},
        steam::WorkshopItem,
    };

    #[test]
    fn round_trips_settings_and_jobs() {
        let mut settings = Settings::new(test_root()).unwrap();
        let storage =
            Storage::initialize(Connection::open_in_memory().unwrap(), &settings).unwrap();
        assert_eq!(storage.settings().unwrap(), settings);
        settings.folder_naming = FolderNaming::Id;
        storage.save_settings(&settings).unwrap();
        assert_eq!(storage.settings().unwrap(), settings);

        assert!(storage.jobs().unwrap().is_empty());
        let mut job = DownloadJob::new(
            WorkshopItem {
                id: "18446744073709551615".into(),
                app_id: 4000,
                title: "Coast".into(),
            },
            &QueueOptions::default(),
            None,
        );
        storage.put_job(&job).unwrap();
        job.status = DownloadStatus::Failed;
        job.error = Some("Provider unavailable".into());
        storage.put_job(&job).unwrap();
        assert_eq!(storage.jobs().unwrap(), vec![job]);
    }

    #[test]
    fn migrates_version_one_settings_and_job_snapshots() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE settings (singleton INTEGER PRIMARY KEY, payload TEXT NOT NULL);
             CREATE TABLE download_jobs (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
             PRAGMA user_version = 1;",
            )
            .unwrap();
        let mut settings = Settings::new(test_root()).unwrap();
        settings.folder_naming = FolderNaming::Id;
        let mut legacy = serde_json::to_value(&settings).unwrap();
        let object = legacy.as_object_mut().unwrap();
        for key in ["backend", "steamcmdPath", "depotDownloaderPath"] {
            object.remove(key);
        }
        connection
            .execute("INSERT INTO settings VALUES (1, ?1)", [legacy.to_string()])
            .unwrap();
        connection.execute("INSERT INTO download_jobs VALUES ('legacy', ?1)", [
            serde_json::json!({
                "id": "legacy", "item": {"id": "42", "appId": 4000, "title": "Coast"},
                "provider": "steamcmd", "destination": settings.download_dir, "status": "running",
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z", "error": null
            }).to_string()
        ]).unwrap();
        let storage =
            Storage::initialize(connection, &Settings::new(test_root()).unwrap()).unwrap();
        assert_eq!(storage.settings().unwrap(), settings);
        assert_eq!(
            storage.job("legacy").unwrap().unwrap().status,
            DownloadStatus::Downloading
        );
        assert!(storage.installed().unwrap().is_empty());
    }

    #[test]
    fn migrates_version_two_and_rolls_back_incomplete_batch_creation() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE settings(singleton INTEGER PRIMARY KEY,payload TEXT NOT NULL);
            CREATE TABLE download_jobs(id TEXT PRIMARY KEY,payload TEXT NOT NULL,queue_order INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE installed_items(app_id INTEGER,item_id TEXT,payload TEXT NOT NULL,PRIMARY KEY(app_id,item_id));
            CREATE TABLE download_logs(id INTEGER PRIMARY KEY,job_id TEXT,time TEXT,message TEXT);
            PRAGMA user_version=2;").unwrap();
        let storage =
            Storage::initialize(connection, &Settings::new(test_root()).unwrap()).unwrap();
        let job = DownloadJob::new(
            WorkshopItem {
                id: "123".into(),
                app_id: 4000,
                title: "Item".into(),
            },
            &QueueOptions::default(),
            None,
        );
        let batch = DownloadBatch {
            id: "batch".into(),
            collection_id: None,
            name: "Test".into(),
            created_at: timestamp(),
            items: vec![],
        };
        assert!(
            storage
                .put_batch(&batch, &[job.clone(), job.clone()])
                .is_err()
        );
        assert!(storage.batches().unwrap().is_empty());
        assert!(storage.jobs().unwrap().is_empty());
        storage.put_batch(&batch, &[job]).unwrap();
        assert_eq!(storage.batches().unwrap().len(), 1);
        assert_eq!(storage.jobs().unwrap().len(), 1);
        assert!(storage.external_items().unwrap().is_empty());
    }

    #[test]
    fn rejects_newer_schema() {
        let connection = Connection::open_in_memory().unwrap();
        connection.pragma_update(None, "user_version", 4).unwrap();
        assert!(Storage::initialize(connection, &Settings::new(test_root()).unwrap()).is_err());
    }
}

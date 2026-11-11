use super::{
    BatchProgress, WorkshopPreview,
    external::{self, ExternalItem, ExternalSummary, ScanReport},
};
use crate::{
    AppError, Result,
    downloads::{DownloadManager, DownloadStatus, QueueOptions},
    steam::{self, SteamClient},
    storage::Storage,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub async fn workshop_is_collection(client: State<'_, SteamClient>, id: String) -> Result<bool> {
    client.is_collection(&id).await
}

#[tauri::command]
pub async fn preview_workshop(
    app: AppHandle,
    client: State<'_, SteamClient>,
    collection_id: Option<String>,
    ids: Vec<String>,
) -> Result<WorkshopPreview> {
    let mut preview = if let Some(id) = collection_id {
        client.preview_collection(&id).await?
    } else {
        client.preview_items(ids).await?
    };
    tauri::async_runtime::spawn_blocking(move || {
        let states = app.state::<DownloadManager>().content_states()?;
        for item in &mut preview.items {
            if let Some(state) = states.get(&item.id) {
                item.status = state.status;
            }
        }
        app.state::<Arc<Storage>>().save_preview(&preview)?;
        Ok(preview)
    })
    .await
    .map_err(|_| AppError::Invalid("Could not prepare the Workshop preview.".into()))?
}

#[tauri::command]
pub async fn queue_workshop_batch(
    app: AppHandle,
    client: State<'_, SteamClient>,
    preview_id: String,
    selected: Vec<String>,
    options: QueueOptions,
    redownload: bool,
) -> Result<super::DownloadBatch> {
    if selected.is_empty() || selected.len() > 2000 {
        return Err(AppError::Invalid(
            "Select between 1 and 2,000 items.".into(),
        ));
    }
    let mut preview = app.state::<Arc<Storage>>().preview(&preview_id)?;
    let allowed: HashSet<_> = preview
        .items
        .iter()
        .filter(|e| e.item.is_some() && e.unavailable.is_none())
        .map(|e| e.id.clone())
        .collect();
    let selected = selected
        .iter()
        .map(|id| steam::validate_id(id))
        .collect::<Result<Vec<_>>>()?;
    if selected.iter().any(|id| !allowed.contains(id)) {
        return Err(AppError::Invalid(
            "Selected items must be available members of this preview.".into(),
        ));
    }
    // Recheck authoritative game/type metadata before admitting jobs to the queue.
    let current = client.preview_items(selected.clone()).await?;
    let current: HashMap<_, _> = current
        .items
        .into_iter()
        .map(|e| (e.id.clone(), e))
        .collect();
    for item in &mut preview.items {
        if let Some(fresh) = current.get(&item.id) {
            if fresh.item.as_ref().map(|i| i.app_id) != item.item.as_ref().map(|i| i.app_id)
                || fresh.unavailable.is_some()
            {
                return Err(AppError::Invalid(format!(
                    "Item #{} changed or became unavailable. Refresh the preview.",
                    item.id
                )));
            }
            *item = fresh.clone();
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<DownloadManager>()
            .enqueue_batch(&preview, &selected, options, redownload)
    })
    .await
    .map_err(|_| AppError::Invalid("Could not queue the batch.".into()))?
}
#[tauri::command]
pub fn list_batches(storage: State<'_, Arc<Storage>>) -> Result<Vec<BatchProgress>> {
    let jobs = storage.jobs()?;
    Ok(storage
        .batches()?
        .into_iter()
        .map(|batch| BatchProgress::new(batch, &jobs))
        .collect())
}
#[tauri::command]
pub fn cancel_batch(manager: State<'_, DownloadManager>, id: String) -> Result<()> {
    manager.cancel_batch(&id)
}
#[tauri::command]
pub async fn retry_batch(app: AppHandle, id: String, options: QueueOptions) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let storage = app.state::<Arc<Storage>>();
        let batch = storage.batch(&id)?;
        let mut failures = vec![];
        for entry in batch.items {
            if let Some(id) = entry.job_id
                && let Some(job) = storage.job(&id)?
                && (job.status == DownloadStatus::Failed
                    || job.status == DownloadStatus::Cancelled
                    || (job.status == DownloadStatus::Queued && job.account_required))
                && let Err(error) = app.state::<DownloadManager>().retry(&id, options.clone())
            {
                failures.push(format!("{}: {error}", entry.title));
            }
        }
        if !failures.is_empty() {
            return Err(AppError::Invalid(failures.join("\n")));
        }
        Ok(())
    })
    .await
    .map_err(|_| AppError::Invalid("Could not retry the batch.".into()))?
}
#[tauri::command]
pub async fn content_states(
    app: AppHandle,
) -> Result<HashMap<String, crate::downloads::ItemDownloadState>> {
    tauri::async_runtime::spawn_blocking(move || app.state::<DownloadManager>().content_states())
        .await
        .map_err(|_| AppError::Invalid("Could not read content states.".into()))?
}
#[tauri::command]
pub async fn list_external_items(app: AppHandle) -> Result<Vec<ExternalSummary>> {
    tauri::async_runtime::spawn_blocking(move || app.state::<DownloadManager>().external_items())
        .await
        .map_err(|_| AppError::Invalid("Could not read external content states.".into()))?
}
#[tauri::command]
pub async fn open_external_folder(app: AppHandle, id: String, path: String) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = app
            .state::<DownloadManager>()
            .open_external_path(&id, &path)?;
        crate::download_commands::open_target(path.as_os_str())
    })
    .await
    .map_err(|_| AppError::Invalid("Could not open external folder.".into()))?
}
#[tauri::command]
pub async fn scan_external(
    app: AppHandle,
    client: State<'_, SteamClient>,
    directory: String,
    workshop_url: Option<String>,
) -> Result<ScanReport> {
    let path = PathBuf::from(directory);
    if !path.is_absolute() {
        return Err(AppError::Invalid(
            "Select an absolute scan directory.".into(),
        ));
    }
    let (candidates, mut warnings) =
        tauri::async_runtime::spawn_blocking(move || external::discover(&path))
            .await
            .map_err(|_| AppError::Invalid("Could not scan that directory.".into()))??;
    let depot = if let Some(url) = workshop_url.filter(|url| !url.trim().is_empty()) {
        let id = steam::item_id(&steam::parse_address(&url)?).ok_or_else(|| {
            AppError::Invalid("Enter a Workshop item URL for the DepotDownloader output.".into())
        })?;
        let identity = client.external_identity(&id).await?;
        Some(identity)
    } else {
        None
    };
    let ids: HashSet<_> = candidates
        .iter()
        .filter_map(|c| c.item_id.clone())
        .collect();
    let metadata = if ids.is_empty() {
        HashMap::new()
    } else {
        client
            .preview_items(ids.into_iter().collect())
            .await?
            .items
            .into_iter()
            .filter_map(|e| e.item.map(|item| (e.id, item)))
            .collect::<HashMap<_, _>>()
    };
    let mut imported = 0;
    let mut recognized = 0;
    let mut seen = HashSet::new();
    for candidate in candidates {
        let item = match &candidate.item_id {
            Some(id) => metadata
                .get(id)
                .filter(|item| item.app_id == candidate.app_id)
                .cloned(),
            None => depot
                .as_ref()
                .filter(|(item, manifest, _)| {
                    item.app_id == candidate.app_id && candidate.manifest.as_ref() == Some(manifest)
                })
                .map(|(item, _, _)| item.clone()),
        };
        let Some(item) = item else {
            warnings.push(format!("{}: no matching public Workshop identity. DepotDownloader output may need its matching Workshop URL.",candidate.path.display()));
            continue;
        };
        if let Err(error) = crate::downloads::files::ensure_directory(&candidate.path, false) {
            warnings.push(error.to_string());
            continue;
        }
        let canonical = match candidate.path.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                warnings.push(format!("{}: {error}", candidate.path.display()));
                continue;
            }
        };
        if !seen.insert((item.id.clone(), canonical.clone())) {
            continue;
        }
        let expected = candidate.expected_bytes.or_else(|| {
            depot
                .as_ref()
                .filter(|(matched, _, _)| matched.id == item.id)
                .and_then(|(_, _, size)| *size)
        });
        if app
            .state::<Arc<Storage>>()
            .installed_item(item.app_id, &item.id)?
            .is_some_and(|installed| std::path::Path::new(&installed.path) == canonical)
        {
            recognized += 1;
            continue;
        }
        let record = tauri::async_runtime::spawn_blocking(move || {
            let record=ExternalItem::capture(item, &canonical, candidate.source)?;
            let total=record.files.iter().try_fold(0u64,|sum,file|sum.checked_add(file.size));
            if expected.is_some_and(|expected|Some(expected)!=total) {return Err(AppError::Invalid("External files do not match the manifest's expected size; incomplete output was not imported.".into()));}
            Ok(record)
        })
        .await
        .map_err(|_| AppError::Invalid("Could not verify external files.".into()))?;
        match record {
            Ok(record) => {
                recognized += 1;
                if app.state::<Arc<Storage>>().save_external(&record)? {
                    imported += 1;
                }
            }
            Err(error) => warnings.push(error.to_string()),
        }
    }
    app.state::<DownloadManager>().invalidate_content();
    Ok(ScanReport {
        imported,
        recognized,
        warnings,
    })
}

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde::Serialize;
use ts_rs::TS;
use zeroize::Zeroizing;

use super::{
    AuthPrompt, BackendChoice, BackendInfo, DownloadJob, DownloadStatus, FailureKind,
    InstalledItem, ProviderError, QueueOptions, files,
    process::Control,
    providers::{self, DownloadProvider, DownloadRequest, Event},
    timestamp, valid_job_id,
};
use crate::{
    AppError, Result,
    settings::Settings,
    steam,
    storage::Storage,
    workshop::{BatchEntry, ContentStatus, DownloadBatch, WorkshopPreview},
};

#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ItemDownloadState {
    pub downloaded: bool,
    pub active: bool,
    pub status: ContentStatus,
    pub path: Option<String>,
    pub source: Option<String>,
}

pub struct DownloadManager {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
    _instance: File,
    cache: Mutex<ContentCache>,
    backend_checks: Mutex<Vec<BackendInfo>>,
}

#[derive(Default)]
struct ContentCache {
    checked: Option<Instant>,
    completed: Vec<String>,
    states: HashMap<String, ItemDownloadState>,
    external: Vec<crate::workshop::external::ExternalSummary>,
}

struct Shared {
    storage: Arc<Storage>,
    root: PathBuf,
    providers: Vec<&'static dyn DownloadProvider>,
    state: Mutex<Scheduler>,
    wake: Condvar,
}

#[derive(Default)]
struct Scheduler {
    active: Option<ActiveJob>,
    accounts: HashMap<String, Login>,
    stopping: bool,
    maintenance: bool,
}

struct ActiveJob {
    id: String,
    control: Arc<Control>,
}

struct Attempt {
    job: DownloadJob,
    control: Arc<Control>,
    login: Option<Login>,
}

#[derive(Clone)]
enum Login {
    Username(Zeroizing<String>),
    Qr,
}

impl Login {
    fn from_options(options: QueueOptions) -> Option<Self> {
        if options.qr == Some(true) {
            Some(Self::Qr)
        } else {
            options
                .account
                .map(|account| Self::Username(Zeroizing::new(account)))
        }
    }

    fn account(&self) -> Option<&str> {
        match self {
            Self::Username(account) => Some(account.as_str()),
            Self::Qr => None,
        }
    }
}

impl DownloadManager {
    pub fn new(storage: Arc<Storage>, root: PathBuf) -> Result<Self> {
        Self::with_providers(storage, root, providers::PROVIDERS.to_vec())
    }

    fn with_providers(
        storage: Arc<Storage>,
        root: PathBuf,
        providers: Vec<&'static dyn DownloadProvider>,
    ) -> Result<Self> {
        files::ensure_directory(&root, true)?;
        files::ensure_directory(&root.join("staging"), true)?;
        files::ensure_directory(&root.join("runtimes"), true)?;
        let instance = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("engine.lock"))?;
        instance.try_lock().map_err(|_| {
            AppError::Invalid(
                "Workshop Downloader’s download engine is already running for this profile.".into(),
            )
        })?;
        let shared = Arc::new(Shared {
            storage,
            root,
            providers,
            state: Mutex::new(Scheduler::default()),
            wake: Condvar::new(),
        });
        shared.recover()?;
        let worker_state = shared.clone();
        let worker = thread::Builder::new()
            .name("workshop-downloads".into())
            .spawn(move || worker_state.work())?;
        Ok(Self {
            shared,
            worker: Mutex::new(Some(worker)),
            _instance: instance,
            cache: Mutex::new(ContentCache::default()),
            backend_checks: Mutex::new(Vec::new()),
        })
    }

    pub fn enqueue(&self, mut job: DownloadJob, options: QueueOptions) -> Result<DownloadJob> {
        options.validate()?;
        let mut state = self.shared.lock()?;
        self.shared.ensure_no_duplicate(&job)?;
        let settings = self.shared.storage.settings()?;
        let previous = self
            .shared
            .storage
            .installed_item(job.item.app_id, &job.item.id)?;
        job.destination =
            path_text(files::choose_destination(&settings, &job.item, previous, job.replace)?.0)?;
        self.shared.storage.put_job(&job)?;
        if let Some(login) = Login::from_options(options) {
            state.accounts.insert(job.id.clone(), login);
        }
        let _ = self
            .shared
            .storage
            .log(&job.id, "Added to the download queue.");
        self.shared.wake.notify_one();
        Ok(job)
    }

    pub fn retry(&self, id: &str, options: QueueOptions) -> Result<()> {
        options.validate()?;
        let mut state = self.shared.lock()?;
        let mut job = self.shared.require_job(id)?;
        let resume = job.status == DownloadStatus::Queued && job.account_required;
        if resume && !options.requires_account() {
            return Err(AppError::Invalid(
                "Enable Steam sign-in to resume this job.".into(),
            ));
        }
        if !resume {
            job.transition(DownloadStatus::Queued)?;
        }
        self.shared.ensure_no_duplicate(&job)?;
        job.requested_backend = options.backend;
        job.provider = options.backend.id().into();
        job.replace = options.replace;
        job.account_required = options.requires_account();
        job.percent = None;
        job.downloaded_bytes = None;
        job.bytes_per_second = None;
        job.error = None;
        job.failure_kind = None;
        job.previous_install = None;
        if resume {
            self.shared.storage.put_job(&job)?;
        } else {
            self.shared.storage.requeue_job(&job)?;
        }
        if let Some(login) = Login::from_options(options) {
            state.accounts.insert(id.into(), login);
        }
        let _ = self.shared.storage.log(id, "Queued another attempt.");
        self.shared.wake.notify_one();
        Ok(())
    }

    pub fn cancel(&self, id: &str) -> Result<()> {
        let mut state = self.shared.lock()?;
        if let Some(active) = &state.active
            && active.id == id
        {
            active.control.cancel();
            return Ok(());
        }
        let mut job = self.shared.require_job(id)?;
        if job.status != DownloadStatus::Queued {
            return Err(AppError::Invalid(
                "This download is no longer in the queue.".into(),
            ));
        }
        job.transition(DownloadStatus::Cancelled)?;
        self.shared.storage.put_job(&job)?;
        state.accounts.remove(id);
        let _ = self.shared.storage.log(id, "Cancelled before starting.");
        self.shared.wake.notify_one();
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let _state = self.shared.lock()?;
        let job = self.shared.require_job(id)?;
        if job.status.is_active() || job.status == DownloadStatus::Queued {
            return Err(AppError::Invalid(
                "Cancel or finish this download before removing it from history.".into(),
            ));
        }
        self.shared.storage.remove_job(id)
    }

    pub fn clear_history(&self) -> Result<()> {
        let state = self.shared.lock()?;
        if state.active.is_some() {
            return Err(AppError::Invalid(
                "Wait for the current download to finish before clearing history.".into(),
            ));
        }
        self.shared.storage.clear_download_history()
    }

    pub fn prompt(&self) -> Result<Option<AuthPrompt>> {
        let state = self.shared.lock()?;
        Ok(state.active.as_ref().and_then(|active| {
            Some(AuthPrompt {
                job_id: active.id.clone(),
                kind: active.control.prompt()?,
                qr: active.control.qr(),
            })
        }))
    }

    pub fn authenticate(&self, id: &str, response: Zeroizing<String>) -> Result<()> {
        let state = self.shared.lock()?;
        if let Some(active) = &state.active
            && active.id == id
            && active.control.respond(response)
        {
            return Ok(());
        }
        Err(AppError::Invalid(
            "The backend is not waiting for that input, or the input is invalid.".into(),
        ))
    }

    pub fn invalidate_content(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.checked = None;
        }
    }

    pub fn content_states(&self) -> Result<HashMap<String, ItemDownloadState>> {
        let jobs = self.shared.storage.jobs()?;
        let completed = jobs
            .iter()
            .filter(|job| job.status == DownloadStatus::Completed)
            .map(|job| job.id.clone())
            .collect::<Vec<_>>();
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| AppError::Invalid("Content status cache is unavailable.".into()))?;
        if cache
            .checked
            .is_none_or(|time| time.elapsed() > Duration::from_secs(5))
            || cache.completed != completed
        {
            let mut states = HashMap::new();
            for item in self.shared.storage.installed()? {
                let valid = files::verified(&item).is_some();
                states.insert(
                    item.item_id,
                    ItemDownloadState {
                        downloaded: valid,
                        active: false,
                        status: if valid {
                            ContentStatus::Downloaded
                        } else {
                            ContentStatus::MissingFiles
                        },
                        path: Some(item.path),
                        source: Some("Workshop Downloader".into()),
                    },
                );
            }
            let mut external = Vec::new();
            for item in self.shared.storage.external_items()? {
                let valid = item.verify();
                external.push(crate::workshop::external::ExternalSummary::new(
                    &item, valid,
                ));
                let state = states.entry(item.item.id).or_insert(ItemDownloadState {
                    downloaded: false,
                    active: false,
                    status: ContentStatus::MissingFiles,
                    path: None,
                    source: None,
                });
                if !state.downloaded && valid {
                    *state = ItemDownloadState {
                        downloaded: true,
                        active: false,
                        status: ContentStatus::DownloadedExternally,
                        path: Some(item.path),
                        source: Some(item.source),
                    };
                }
            }
            cache.external = external;
            cache.states = states;
            cache.completed = completed;
            cache.checked = Some(Instant::now());
        }
        let mut states = cache.states.clone();
        let mut seen = HashSet::new();
        // jobs() is newest first: preserve the latest terminal failure for each item.
        for job in jobs {
            let state = states
                .entry(job.item.id.clone())
                .or_insert_with(empty_content_state);
            if job.status == DownloadStatus::Queued || job.status.is_active() {
                state.active = true;
                state.status = if job.status == DownloadStatus::Queued {
                    ContentStatus::Queued
                } else {
                    ContentStatus::Downloading
                };
            } else if seen.insert(job.item.id)
                && job.status == DownloadStatus::Failed
                && !state.downloaded
                && !state.active
                && state.status != ContentStatus::MissingFiles
            {
                state.status = ContentStatus::Failed;
            }
        }
        Ok(states)
    }
    pub fn external_items(&self) -> Result<Vec<crate::workshop::external::ExternalSummary>> {
        self.content_states()?;
        Ok(self
            .cache
            .lock()
            .map_err(|_| AppError::Invalid("Content cache is unavailable.".into()))?
            .external
            .clone())
    }
    pub fn open_external_path(&self, id: &str, path: &str) -> Result<PathBuf> {
        let record = self
            .shared
            .storage
            .external_items()?
            .into_iter()
            .find(|item| item.item.id == id && item.path == path)
            .filter(|item| item.verify())
            .ok_or_else(|| {
                AppError::Invalid(
                    "The external files are missing or have changed. Rescan the directory.".into(),
                )
            })?;
        Ok(PathBuf::from(record.path))
    }
    pub fn state_for(&self, item_id: &str) -> Result<ItemDownloadState> {
        let id = steam::validate_id(item_id)?;
        Ok(self
            .content_states()?
            .remove(&id)
            .unwrap_or_else(empty_content_state))
    }
    pub fn open_item_path(&self, id: &str) -> Result<PathBuf> {
        self.invalidate_content();
        let state = self.state_for(id)?;
        if state.downloaded
            && let Some(path) = state.path
        {
            return Ok(PathBuf::from(path));
        }
        Err(AppError::Invalid(
            "Workshop Downloader could not find verified files for this item.".into(),
        ))
    }

    pub fn enqueue_batch(
        &self,
        preview: &WorkshopPreview,
        selected: &[String],
        options: QueueOptions,
        redownload: bool,
    ) -> Result<DownloadBatch> {
        options.validate()?;
        if options.replace {
            return Err(AppError::Invalid("Batch downloads use fresh folders; replacement is only available for individual retries.".into()));
        }
        if selected.is_empty() || selected.len() > 2000 {
            return Err(AppError::Invalid(
                "Select between 1 and 2,000 items.".into(),
            ));
        }
        self.invalidate_content();
        let statuses = self.content_states()?;
        let mut scheduler = self.shared.lock()?;
        let settings = self.shared.storage.settings()?;
        let mut batch = DownloadBatch {
            id: uuid::Uuid::new_v4().to_string(),
            collection_id: preview.collection_id.clone(),
            name: preview.name.clone(),
            created_at: timestamp(),
            items: vec![],
        };
        let mut jobs = vec![];
        let mut seen = HashSet::new();
        for id in selected {
            let id = steam::validate_id(id)?;
            if !seen.insert(id.clone()) {
                continue;
            }
            let entry = preview
                .items
                .iter()
                .find(|entry| entry.id == id)
                .ok_or_else(|| {
                    AppError::Invalid("An item was not included in the verified preview.".into())
                })?;
            let item = entry
                .item
                .as_ref()
                .filter(|item| {
                    item.id == id
                        && item.app_id > 0
                        && preview.app_id.is_none_or(|app| app == item.app_id)
                })
                .ok_or_else(|| {
                    AppError::Invalid(format!(
                        "Workshop item #{id} is unavailable or belongs to another game."
                    ))
                })?;
            let state = statuses.get(&id);
            let reason = if state.is_some_and(|state| state.active) {
                Some("Already queued")
            } else if !redownload && state.is_some_and(|state| state.downloaded) {
                Some("Already downloaded")
            } else {
                None
            };
            let mut member = BatchEntry {
                item_id: id,
                title: item.title.clone(),
                job_id: None,
                skipped: reason.is_some(),
                reason: reason.map(str::to_owned),
            };
            if reason.is_none() {
                let mut job = DownloadJob::new(item.clone(), &options, entry.updated_at);
                job.expected_bytes = entry.size_bytes;
                self.shared.ensure_no_duplicate(&job)?;
                job.destination = path_text(
                    files::choose_destination(
                        &settings,
                        item,
                        self.shared.storage.installed_item(item.app_id, &item.id)?,
                        false,
                    )?
                    .0,
                )?;
                member.job_id = Some(job.id.clone());
                jobs.push(job);
            }
            batch.items.push(member);
        }
        // Jobs and batch membership commit together before the one existing worker wakes.
        self.shared.storage.put_batch(&batch, &jobs)?;
        if let Some(login) = Login::from_options(options) {
            for job in &jobs {
                scheduler.accounts.insert(job.id.clone(), login.clone());
            }
        }
        self.shared.wake.notify_one();
        Ok(batch)
    }
    pub fn cancel_batch(&self, id: &str) -> Result<()> {
        let mut scheduler = self.shared.lock()?;
        let batch = self.shared.storage.batch(id)?;
        for entry in batch.items {
            let Some(id) = entry.job_id else {
                continue;
            };
            if let Some(active) = &scheduler.active
                && active.id == id
            {
                active.control.cancel();
                continue;
            }
            if let Some(mut job) = self.shared.storage.job(&id)?
                && job.status == DownloadStatus::Queued
            {
                job.transition(DownloadStatus::Cancelled)?;
                self.shared.storage.put_job(&job)?;
                scheduler.accounts.remove(&id);
            }
        }
        self.shared.wake.notify_one();
        Ok(())
    }
    pub fn open_job_path(&self, id: &str) -> Result<PathBuf> {
        let job = self.shared.require_job(id)?;
        let item = InstalledItem {
            job_id: job.id,
            app_id: job.item.app_id,
            item_id: job.item.id,
            path: job.destination,
        };
        if job.status != DownloadStatus::Completed || files::verified(&item).is_none() {
            return Err(AppError::Invalid(
                "The files for this download are missing or were replaced.".into(),
            ));
        }
        Ok(PathBuf::from(item.path))
    }

    pub fn backend_info(&self, settings: &Settings, check: bool) -> Result<Vec<BackendInfo>> {
        let tools = self.shared.root.join("tools");
        if !check {
            let mut backends = providers::inspect(settings, &tools, None);
            if let Ok(checks) = self.backend_checks.lock() {
                for backend in &mut backends {
                    if let Some(check) = checks.iter().find(|check| {
                        check.backend == backend.backend && check.path == backend.path
                    }) {
                        backend.version.clone_from(&check.version);
                        backend.state = check.state;
                        backend.message.clone_from(&check.message);
                    }
                }
            }
            return Ok(backends);
        }
        let _maintenance = self.reserve_maintenance()?;
        let workspace = self
            .shared
            .root
            .join("checks")
            .join(uuid::Uuid::new_v4().to_string());
        files::ensure_directory(&workspace, true)?;
        let result = providers::inspect(settings, &tools, Some(&workspace));
        let _ = fs::remove_dir_all(workspace);
        if let Ok(mut checks) = self.backend_checks.lock() {
            checks.clone_from(&result);
        }
        Ok(result)
    }

    pub(crate) fn backend_release(&self, backend: BackendChoice) -> Result<super::BackendRelease> {
        super::tools::check(&self.shared.root.join("tools"), backend)
    }

    pub(crate) fn install_backend(&self, backend: BackendChoice, source: &str) -> Result<()> {
        let _maintenance = self.reserve_maintenance()?;
        super::tools::install(&self.shared.root.join("tools"), backend, source)
    }

    pub(crate) fn restore_backend(&self, backend: BackendChoice) -> Result<()> {
        let _maintenance = self.reserve_maintenance()?;
        super::tools::restore(&self.shared.root.join("tools"), backend)
    }

    pub(crate) fn clear_backend_session(&self, backend: BackendChoice) -> Result<()> {
        let _maintenance = self.reserve_maintenance()?;
        if backend == BackendChoice::Automatic {
            return Err(AppError::Invalid("Select a download engine.".into()));
        }
        if cfg!(windows) && backend == BackendChoice::DepotDownloader {
            return Err(AppError::Invalid("DepotDownloader uses Windows .NET account storage. Use Steam’s account security controls to revoke its session.".into()));
        }
        let path = self.shared.root.join("runtimes").join(backend.id());
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                files::ensure_directory(&path, false)?;
                fs::remove_dir_all(path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    pub(crate) fn reserve_maintenance(&self) -> Result<Maintenance<'_>> {
        let mut state = self.shared.lock()?;
        if state.active.is_some() || state.maintenance || state.stopping {
            return Err(AppError::Invalid(
                "Wait for the current download or setup operation to finish.".into(),
            ));
        }
        state.maintenance = true;
        Ok(Maintenance(&self.shared))
    }

    pub fn shutdown(&self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.stopping = true;
            if let Some(active) = &state.active {
                active.control.cancel();
            }
            self.shared.wake.notify_all();
        }
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

pub(crate) struct Maintenance<'a>(&'a Shared);

impl Drop for Maintenance<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.maintenance = false;
            self.0.wake.notify_all();
        }
    }
}

impl Drop for DownloadManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn empty_content_state() -> ItemDownloadState {
    ItemDownloadState {
        downloaded: false,
        active: false,
        status: ContentStatus::NotDownloaded,
        path: None,
        source: None,
    }
}

impl Shared {
    fn lock(&self) -> Result<MutexGuard<'_, Scheduler>> {
        self.state
            .lock()
            .map_err(|_| AppError::Invalid("The download queue is unavailable.".into()))
    }

    fn require_job(&self, id: &str) -> Result<DownloadJob> {
        self.storage
            .job(id)?
            .ok_or_else(|| AppError::Invalid("Download record not found.".into()))
    }

    fn ensure_no_duplicate(&self, job: &DownloadJob) -> Result<()> {
        if self.storage.jobs()?.iter().any(|current| {
            current.id != job.id
                && current.item.id == job.item.id
                && current.item.app_id == job.item.app_id
                && (current.status == DownloadStatus::Queued || current.status.is_active())
        }) {
            return Err(AppError::Invalid(
                "This Workshop item is already in the download queue.".into(),
            ));
        }
        Ok(())
    }

    fn next(&self) -> Result<Option<Attempt>> {
        let mut state = self.lock()?;
        loop {
            if state.stopping {
                return Ok(None);
            }
            if !state.maintenance
                && let Some(mut job) = self
                    .storage
                    .queued_jobs()?
                    .into_iter()
                    .find(|job| !job.account_required || state.accounts.contains_key(&job.id))
            {
                let login = state.accounts.remove(&job.id);
                job.transition(DownloadStatus::Preparing)?;
                self.storage.put_job(&job)?;
                let control = Arc::new(Control::default());
                state.active = Some(ActiveJob {
                    id: job.id.clone(),
                    control: control.clone(),
                });
                return Ok(Some(Attempt {
                    job,
                    control,
                    login,
                }));
            }
            state = self
                .wake
                .wait(state)
                .map_err(|_| AppError::Invalid("The download queue is unavailable.".into()))?;
        }
    }

    fn work(&self) {
        loop {
            match self.next() {
                Ok(Some(Attempt {
                    mut job,
                    control,
                    login,
                })) => {
                    self.run_job(&mut job, &control, login.as_ref());
                    if let Ok(mut state) = self.lock() {
                        state.active = None;
                    }
                }
                Ok(None) => return,
                Err(error) => {
                    eprintln!("Workshop queue stopped: {error}");
                    return;
                }
            }
        }
    }

    fn run_job(&self, job: &mut DownloadJob, control: &Control, login: Option<&Login>) {
        if !valid_job_id(&job.id)
            || job.item.app_id == 0
            || !job
                .item
                .id
                .parse::<u64>()
                .is_ok_and(|id| id > 0 && id.to_string() == job.item.id)
        {
            let _ = self.record_failure(
                job,
                ProviderError::new(
                    FailureKind::InvalidItem,
                    "The queued Workshop IDs are invalid.",
                ),
                false,
            );
            return;
        }
        let stage = self.root.join("staging").join(&job.id);
        let result = self.attempt(job, &stage, control, login);
        let result = match result {
            Ok((installation, bytes)) => self.finish(job, &installation, bytes),
            Err(error) if job.status == DownloadStatus::Finalizing => {
                match files::recover_publication(job) {
                    Ok(Some((installation, bytes))) => self.finish(job, &installation, bytes),
                    Ok(None) => self.record_failure(job, error, control.cancelled()),
                    Err(recovery) => self.record_failure(job, from_files(recovery), false),
                }
            }
            Err(error) => self.record_failure(job, error, control.cancelled()),
        };
        if let Err(error) = result {
            eprintln!("Could not store final download status: {error}");
        }
        if stage.exists()
            && let Err(error) = fs::remove_dir_all(&stage)
        {
            let _ = self.storage.log(
                &job.id,
                &format!("Could not remove the staging directory. {error}"),
            );
        }
    }

    fn attempt(
        &self,
        job: &mut DownloadJob,
        stage: &Path,
        control: &Control,
        login: Option<&Login>,
    ) -> std::result::Result<(InstalledItem, u64), ProviderError> {
        let settings = self.storage.settings().map_err(from_files)?;
        let candidates = providers::eligible(
            &self.providers,
            &settings,
            job.requested_backend,
            login.is_some(),
            &self.root.join("tools"),
        );
        if candidates.is_empty() {
            return Err(ProviderError::new(
                FailureKind::BackendUnavailable,
                format!(
                    "No eligible {} backend is available. Install a backend or set its executable in Preferences.",
                    job.requested_backend.name()
                ),
            ));
        }
        let previous = self
            .storage
            .installed_item(job.item.app_id, &job.item.id)
            .map_err(from_files)?;
        let (path, previous) =
            files::choose_destination(&settings, &job.item, previous, job.replace)
                .map_err(from_files)?;
        let parent = path.parent().ok_or_else(|| {
            from_files(AppError::Invalid(
                "The download has no destination directory.".into(),
            ))
        })?;
        if parent.starts_with(&self.root) {
            return Err(from_files(AppError::Invalid(
                "Choose a download destination outside Workshop Downloader’s staging directory."
                    .into(),
            )));
        }
        files::ensure_directory(parent, true).map_err(from_files)?;
        job.destination = path_text(path).map_err(from_files)?;
        job.previous_install = previous;
        let item = job.item.clone();
        let runtime = self.root.join("runtimes");

        for (index, candidate) in candidates.iter().enumerate() {
            if control.cancelled() {
                return Err(ProviderError::new(
                    FailureKind::Process,
                    "Download cancelled.",
                ));
            }
            if index > 0 {
                fs::remove_dir_all(stage).map_err(|error| from_files(error.into()))?;
                job.transition(DownloadStatus::Preparing)
                    .map_err(from_files)?;
                job.percent = None;
                job.downloaded_bytes = None;
            }
            files::create_private_directory(stage).map_err(|error| from_files(error.into()))?;
            let backend = candidate.provider.id();
            job.provider = backend.id().into();
            self.storage
                .log(&job.id, &format!("Starting {}.", backend.name()))
                .map_err(from_files)?;
            job.transition(if login.is_some() {
                DownloadStatus::Authenticating
            } else {
                DownloadStatus::Downloading
            })
            .map_err(from_files)?;
            self.storage.put_job(job).map_err(from_files)?;

            let request = DownloadRequest {
                item: &item,
                stage,
                runtime: &runtime,
                account: login.and_then(Login::account),
                qr: matches!(login, Some(Login::Qr)),
            };
            let mut event_error = None;
            let mut last_progress = Instant::now();
            let mut report = |event| {
                if event_error.is_none() {
                    event_error = self.record_event(job, event, &mut last_progress).err();
                    if event_error.is_some() {
                        control.cancel();
                    }
                }
            };
            let mut result =
                candidate
                    .provider
                    .download(&candidate.executable, &request, control, &mut report);
            if login.is_none()
                && !control.cancelled()
                && result
                    .as_ref()
                    .is_err_and(|error| error.kind == FailureKind::Authentication)
            {
                job.transition(DownloadStatus::Authenticating)
                    .map_err(from_files)?;
                self.storage.put_job(job).map_err(from_files)?;
                self.storage
                    .log(&job.id, "The backend requires Steam sign-in.")
                    .map_err(from_files)?;
                let account = control.wait_for_account()?;
                fs::remove_dir_all(stage).map_err(|error| from_files(error.into()))?;
                files::create_private_directory(stage).map_err(|error| from_files(error.into()))?;
                let request = DownloadRequest {
                    account: Some(account.as_str()),
                    ..request
                };
                result = candidate.provider.download(
                    &candidate.executable,
                    &request,
                    control,
                    &mut |event| {
                        if event_error.is_none() {
                            event_error = self.record_event(job, event, &mut last_progress).err();
                            if event_error.is_some() {
                                control.cancel();
                            }
                        }
                    },
                );
                if let Err(error) = result {
                    // Keep authentication and ownership restrictions on the engine that reported them.
                    return Err(error);
                }
            }
            if let Some(error) = event_error {
                return Err(from_files(error));
            }
            match result {
                Ok(payload) => {
                    if control.cancelled() {
                        return Err(ProviderError::new(
                            FailureKind::Process,
                            "Download cancelled.",
                        ));
                    }
                    files::verify_payload(&payload).map_err(from_files)?;
                    if job.status == DownloadStatus::Authenticating {
                        job.transition(DownloadStatus::Downloading)
                            .map_err(from_files)?;
                    }
                    job.transition(DownloadStatus::Finalizing)
                        .map_err(from_files)?;
                    self.storage.put_job(job).map_err(from_files)?;
                    self.storage
                        .log(
                            &job.id,
                            "Verified staged files. Publishing to the selected folder.",
                        )
                        .map_err(from_files)?;
                    let result = files::publish(job, &payload, control).map_err(from_files);
                    if result.is_ok() && payload.path.starts_with(&runtime) {
                        let _ = fs::remove_dir_all(&payload.path);
                    }
                    return result;
                }
                Err(error)
                    if !control.cancelled()
                        && index + 1 < candidates.len()
                        && error.kind.can_fallback() =>
                {
                    self.storage
                        .log(
                            &job.id,
                            &format!("{} Trying the next eligible backend.", error.message),
                        )
                        .map_err(from_files)?;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the last eligible backend always returns");
    }

    fn record_event(
        &self,
        job: &mut DownloadJob,
        event: Event,
        last_progress: &mut Instant,
    ) -> Result<()> {
        match event {
            Event::Downloading if job.status == DownloadStatus::Authenticating => {
                job.transition(DownloadStatus::Downloading)?;
                self.storage.put_job(job)?;
            }
            Event::Downloading => {}
            Event::Percent(percent) => {
                job.percent = Some(percent);
                if last_progress.elapsed() >= Duration::from_millis(350) {
                    job.updated_at = timestamp();
                    self.storage.put_job(job)?;
                    *last_progress = Instant::now();
                }
            }
            Event::Bytes(bytes) => {
                job.downloaded_bytes = Some(bytes);
                self.storage.put_job(job)?;
            }
            Event::Note(message) => self.storage.log(&job.id, message)?,
            Event::Exit(code) => self
                .storage
                .log(&job.id, &format!("Backend exited with code {code}."))?,
        }
        Ok(())
    }

    fn finish(&self, job: &mut DownloadJob, item: &InstalledItem, bytes: u64) -> Result<()> {
        job.transition(DownloadStatus::Completed)?;
        job.downloaded_at = Some(job.updated_at.clone());
        job.downloaded_bytes = Some(bytes);
        job.percent = Some(100.0);
        self.storage.complete_job(job, item)?;
        self.storage.log(
            &job.id,
            "Download complete. Files are available in the destination folder.",
        )?;
        if let Err(error) = files::cleanup_backup(job) {
            let _ = self.storage.log(
                &job.id,
                &format!("The replacement completed, but its backup could not be removed. {error}"),
            );
        }
        Ok(())
    }

    fn record_failure(
        &self,
        job: &mut DownloadJob,
        error: ProviderError,
        cancelled: bool,
    ) -> Result<()> {
        job.transition(if cancelled {
            DownloadStatus::Cancelled
        } else {
            DownloadStatus::Failed
        })?;
        if !cancelled {
            job.error = Some(error.message.clone());
            job.failure_kind = Some(error.kind);
        }
        self.storage.put_job(job)?;
        self.storage.log(
            &job.id,
            if cancelled {
                "Download cancelled."
            } else {
                &error.message
            },
        )
    }

    fn recover(&self) -> Result<()> {
        for mut job in self.storage.jobs()? {
            if job.status == DownloadStatus::Completed && job.previous_install.is_some() {
                let _ = files::cleanup_backup(&job);
            }
            if !job.status.is_active() {
                continue;
            }
            if job.status == DownloadStatus::Finalizing {
                match files::recover_publication(&job) {
                    Ok(Some((item, bytes))) => {
                        self.finish(&mut job, &item, bytes)?;
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.record_failure(&mut job, from_files(error), false)?;
                        continue;
                    }
                }
            }
            self.record_failure(&mut job, ProviderError::new(FailureKind::Interrupted,
                "Workshop Downloader closed before this download finished. Retry to start a fresh transfer."), false)?;
        }

        let staging = self.root.join("staging");
        for entry in fs::read_dir(&staging)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok())
            {
                fs::remove_dir_all(entry.path())?;
            }
        }
        Ok(())
    }
}

fn path_text(path: PathBuf) -> Result<String> {
    path.into_os_string().into_string().map_err(|_| {
        AppError::Invalid("The destination path cannot be represented as text.".into())
    })
}

fn from_files(error: AppError) -> ProviderError {
    ProviderError::new(FailureKind::Files, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::downloads::BackendChoice;
    use crate::steam::WorkshopItem;

    fn fixture() -> (tempfile::TempDir, Arc<Storage>, Settings) {
        let root = tempfile::tempdir().unwrap();
        let settings = Settings::new(root.path().join("Downloads")).unwrap();
        let storage =
            Arc::new(Storage::open(&root.path().join("test.sqlite3"), &settings).unwrap());
        (root, storage, settings)
    }

    fn job(id: &str, options: &QueueOptions) -> DownloadJob {
        DownloadJob::new(
            WorkshopItem {
                id: id.into(),
                app_id: 294100,
                title: format!("Mod {id}"),
            },
            options,
            None,
        )
    }

    #[cfg(unix)]
    fn wait_for(mut check: impl FnMut() -> bool) {
        let start = Instant::now();
        while !check() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out waiting for the download queue"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn batches_skip_verified_external_content_and_keep_relations_across_restart() {
        let (root, storage, settings) = fixture();
        let external_path = root.path().join("external");
        fs::create_dir(&external_path).unwrap();
        fs::write(external_path.join("payload.lua"), "verified").unwrap();
        let item = job("123", &QueueOptions::default()).item;
        let external = crate::workshop::external::ExternalItem::capture(
            item.clone(),
            &external_path,
            "Test external".into(),
        )
        .unwrap();
        assert!(storage.save_external(&external).unwrap());
        assert!(!storage.save_external(&external).unwrap());
        let preview = WorkshopPreview {
            id: "preview".into(),
            collection_id: Some("987".into()),
            name: "Collection".into(),
            app_id: Some(294100),
            items: vec![crate::workshop::PreviewEntry {
                id: "123".into(),
                item: Some(item),
                updated_at: None,
                size_bytes: None,
                unavailable: None,
                status: ContentStatus::DownloadedExternally,
            }],
            warnings: vec![],
        };
        storage.save_preview(&preview).unwrap();
        storage.save_preview(&preview).unwrap();
        let manager =
            DownloadManager::with_providers(storage.clone(), root.path().join("engine"), vec![])
                .unwrap();
        let batch = manager
            .enqueue_batch(
                &preview,
                &["123".into(), "123".into()],
                QueueOptions::default(),
                false,
            )
            .unwrap();
        assert_eq!(batch.items.len(), 1);
        assert!(batch.items[0].skipped);
        assert!(storage.jobs().unwrap().is_empty());
        assert_eq!(
            manager.state_for("123").unwrap().status,
            ContentStatus::DownloadedExternally
        );
        fs::remove_file(external_path.join("payload.lua")).unwrap();
        manager.invalidate_content();
        assert_eq!(
            manager.state_for("123").unwrap().status,
            ContentStatus::MissingFiles
        );
        manager.shutdown();
        drop(manager);
        drop(storage);
        let reopened = Storage::open(&root.path().join("test.sqlite3"), &settings).unwrap();
        assert_eq!(reopened.batch(&batch.id).unwrap().items.len(), 1);
        assert_eq!(reopened.external_items().unwrap().len(), 1);
        assert_eq!(
            reopened
                .preview("preview")
                .unwrap()
                .collection_id
                .as_deref(),
            Some("987")
        );
    }
    #[test]
    fn batch_cancellation_and_partial_failure_retry_reuse_existing_jobs() {
        let (root, storage, _) = fixture();
        let mut first = job("123", &QueueOptions::default());
        first.status = DownloadStatus::Failed;
        let mut second = job("234", &QueueOptions::default());
        second.account_required = true;
        let batch = DownloadBatch {
            id: uuid::Uuid::new_v4().to_string(),
            collection_id: None,
            name: "Selection".into(),
            created_at: timestamp(),
            items: vec![
                BatchEntry {
                    item_id: first.item.id.clone(),
                    title: first.item.title.clone(),
                    job_id: Some(first.id.clone()),
                    skipped: false,
                    reason: None,
                },
                BatchEntry {
                    item_id: second.item.id.clone(),
                    title: second.item.title.clone(),
                    job_id: Some(second.id.clone()),
                    skipped: false,
                    reason: None,
                },
            ],
        };
        storage
            .put_batch(&batch, &[first.clone(), second.clone()])
            .unwrap();
        let manager =
            DownloadManager::with_providers(storage.clone(), root.path().join("engine"), vec![])
                .unwrap();
        manager.cancel_batch(&batch.id).unwrap();
        let progress = crate::workshop::BatchProgress::new(
            storage.batch(&batch.id).unwrap(),
            &storage.jobs().unwrap(),
        );
        assert_eq!(progress.failed, 1);
        assert_eq!(progress.cancelled, 1);
        assert_eq!(progress.completed, 0);
        let options = QueueOptions {
            account: Some("account".into()),
            ..Default::default()
        };
        manager.retry(&first.id, options.clone()).unwrap();
        manager.retry(&second.id, options).unwrap();
        let progress = crate::workshop::BatchProgress::new(
            storage.batch(&batch.id).unwrap(),
            &storage.jobs().unwrap(),
        );
        assert_eq!(progress.total, 2);
        assert_eq!(progress.cancelled, 0);
        assert_eq!(storage.jobs().unwrap().len(), 2);
        assert!(storage.remove_job(&first.id).is_err());
        manager.shutdown();
    }

    #[test]
    fn queued_sign_in_and_interrupted_jobs_are_recovered_separately() {
        let (root, storage, settings) = fixture();
        let options = QueueOptions {
            account: Some("account".into()),
            ..Default::default()
        };
        let first = job("123", &options);
        let second = job("234", &options);
        let mut interrupted = job("345", &QueueOptions::default());
        interrupted.transition(DownloadStatus::Preparing).unwrap();
        interrupted.transition(DownloadStatus::Downloading).unwrap();
        for job in [&first, &second, &interrupted] {
            storage.put_job(job).unwrap();
        }
        drop(storage);

        let storage =
            Arc::new(Storage::open(&root.path().join("test.sqlite3"), &settings).unwrap());
        let manager =
            DownloadManager::with_providers(storage.clone(), root.path().join("engine"), vec![])
                .unwrap();
        assert_eq!(
            storage
                .queued_jobs()
                .unwrap()
                .iter()
                .map(|job| &job.id)
                .collect::<Vec<_>>(),
            [&first.id, &second.id]
        );
        let interrupted = storage.job(&interrupted.id).unwrap().unwrap();
        assert_eq!(interrupted.status, DownloadStatus::Failed);
        assert_eq!(interrupted.failure_kind, Some(FailureKind::Interrupted));
        manager.cancel(&first.id).unwrap();
        assert_eq!(
            storage.job(&first.id).unwrap().unwrap().status,
            DownloadStatus::Cancelled
        );
        let mut retried = storage.job(&first.id).unwrap().unwrap();
        retried.transition(DownloadStatus::Queued).unwrap();
        storage.requeue_job(&retried).unwrap();
        assert_eq!(
            storage
                .queued_jobs()
                .unwrap()
                .iter()
                .map(|job| &job.id)
                .collect::<Vec<_>>(),
            [&second.id, &first.id]
        );
        manager.shutdown();
    }

    #[cfg(unix)]
    struct MockCli;

    #[cfg(unix)]
    impl DownloadProvider for MockCli {
        fn id(&self) -> BackendChoice {
            BackendChoice::DepotDownloader
        }
        fn detect(&self, _settings: &Settings) -> Option<PathBuf> {
            Some("/bin/sh".into())
        }
        fn probe(
            &self,
            _executable: &Path,
            _workspace: &Path,
        ) -> std::result::Result<Option<String>, ProviderError> {
            Ok(None)
        }
        fn download(
            &self,
            executable: &Path,
            request: &DownloadRequest<'_>,
            control: &Control,
            report: &mut dyn FnMut(Event),
        ) -> std::result::Result<providers::Payload, ProviderError> {
            if request.item.id == "104" && request.account.is_none() {
                return Err(ProviderError::new(
                    FailureKind::Authentication,
                    "Sign-in required.",
                ));
            }
            let mut command = portable_pty::CommandBuilder::new(executable);
            command.cwd(request.stage);
            let script = match request.item.id.as_str() {
                "101" => "printf 'Network failure\\n'; exit 7",
                "102" => "sleep 45 & echo $! > child.pid; wait",
                "103" | "104" => {
                    "printf 'password: '; read -r secret; test \"$secret\" = 'temporary-test-password' || exit 8; printf '\\nSteam Guard code: '; read -r guard; test \"$guard\" = 'TST42' || exit 9; printf '\\nreceived\\n'; mkdir content; printf mod > content/mod.txt"
                }
                _ => "mkdir content; printf mod > content/mod.txt",
            };
            command.args(["-c", script]);
            let status = super::super::process::run(
                command,
                "Test engine".into(),
                control,
                Duration::from_secs(8),
                |line, _| {
                    if line.trim() == "password:" {
                        control.set_prompt(Some(super::super::PromptKind::Password));
                    } else if line.trim() == "Steam Guard code:" {
                        control.set_prompt(Some(super::super::PromptKind::GuardCode));
                    } else if line == "received" {
                        report(Event::Downloading);
                    }
                },
            )?;
            if !status.success() {
                return Err(ProviderError::new(
                    FailureKind::Network,
                    "Mock CLI connection failed.",
                ));
            }
            Ok(providers::Payload {
                path: request.stage.join("content"),
                exclude: None,
                reported_bytes: Some(3),
            })
        }
    }

    #[cfg(unix)]
    #[test]
    fn anonymous_authentication_failure_holds_and_resumes_the_same_job() {
        let (root, storage, _) = fixture();
        let manager = DownloadManager::with_providers(
            storage.clone(),
            root.path().join("engine"),
            vec![&MockCli],
        )
        .unwrap();
        let options = QueueOptions::default();
        let queued = manager.enqueue(job("104", &options), options).unwrap();
        use super::super::PromptKind;
        for (kind, value) in [
            (PromptKind::Account, "account"),
            (PromptKind::Password, "temporary-test-password"),
            (PromptKind::GuardCode, "TST42"),
        ] {
            wait_for(|| {
                manager
                    .prompt()
                    .unwrap()
                    .is_some_and(|prompt| prompt.kind == kind)
            });
            manager
                .authenticate(&queued.id, Zeroizing::new(value.into()))
                .unwrap();
        }
        wait_for(|| storage.job(&queued.id).unwrap().unwrap().status == DownloadStatus::Completed);
        assert_eq!(storage.jobs().unwrap().len(), 1);
        assert_eq!(storage.installed().unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn mock_cli_reports_failure_and_cancels_its_process_tree() {
        let (root, storage, _) = fixture();
        let engine = root.path().join("engine");
        let manager =
            DownloadManager::with_providers(storage.clone(), engine.clone(), vec![&MockCli])
                .unwrap();
        let options = QueueOptions::default();
        let failing = manager
            .enqueue(job("101", &options), options.clone())
            .unwrap();
        let running = manager.enqueue(job("102", &options), options).unwrap();
        wait_for(|| storage.job(&failing.id).unwrap().unwrap().status == DownloadStatus::Failed);
        assert_eq!(
            storage.job(&failing.id).unwrap().unwrap().failure_kind,
            Some(FailureKind::Network)
        );
        let pid_file = engine.join("staging").join(&running.id).join("child.pid");
        wait_for(|| {
            fs::read_to_string(&pid_file).is_ok_and(|pid| pid.trim().parse::<u32>().is_ok())
        });
        let pid = fs::read_to_string(&pid_file).unwrap();
        manager.cancel(&running.id).unwrap();
        wait_for(|| storage.job(&running.id).unwrap().unwrap().status == DownloadStatus::Cancelled);
        wait_for(|| {
            fs::read_to_string(format!("/proc/{}/stat", pid.trim())).map_or(true, |stat| {
                stat.rsplit_once(')')
                    .is_some_and(|(_, state)| state.trim_start().starts_with('Z'))
            })
        });
        assert!(storage.installed().unwrap().is_empty());
        manager.retry(&failing.id, QueueOptions::default()).unwrap();
        wait_for(|| {
            storage
                .logs(&failing.id)
                .unwrap()
                .iter()
                .filter(|log| log.message == "Mock CLI connection failed.")
                .count()
                == 2
        });
        assert_eq!(
            storage.job(&failing.id).unwrap().unwrap().status,
            DownloadStatus::Failed
        );
    }

    #[cfg(unix)]
    #[test]
    fn mock_cli_authenticates_without_logging_secrets_and_keeps_installs_after_history_removal() {
        let (root, storage, _) = fixture();
        let manager = DownloadManager::with_providers(
            storage.clone(),
            root.path().join("engine"),
            vec![&MockCli],
        )
        .unwrap();
        let options = QueueOptions {
            account: Some("account".into()),
            ..Default::default()
        };
        let queued = manager.enqueue(job("103", &options), options).unwrap();
        wait_for(|| manager.prompt().unwrap().is_some());
        assert!(
            manager
                .authenticate(&queued.id, Zeroizing::new("temporary-test-password".into()))
                .is_ok()
        );
        wait_for(|| {
            manager
                .prompt()
                .unwrap()
                .is_some_and(|prompt| prompt.kind == super::super::PromptKind::GuardCode)
        });
        manager
            .authenticate(&queued.id, Zeroizing::new("TST42".into()))
            .unwrap();
        wait_for(|| storage.job(&queued.id).unwrap().unwrap().status == DownloadStatus::Completed);
        assert!(
            storage
                .logs(&queued.id)
                .unwrap()
                .iter()
                .all(|line| !line.message.contains("temporary-test-password")
                    && !line.message.contains("TST42"))
        );
        assert!(manager.state_for("103").unwrap().downloaded);
        manager.remove(&queued.id).unwrap();
        assert!(storage.job(&queued.id).unwrap().is_none());
        assert!(manager.state_for("103").unwrap().downloaded);
    }
}

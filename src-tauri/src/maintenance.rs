use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};
use ts_rs::TS;
use url::Url;

use crate::{
    AppError, Result,
    downloads::{BackendChoice, DownloadManager},
    storage::Storage,
};

const UPDATER_KEY: Option<&str> = option_env!("WORKSHOP_UPDATER_PUBLIC_KEY");
const UPDATE_FEED: &str =
    "https://github.com/NotYetFound/Workshop-Downloader/releases/latest/download/latest.json";

#[derive(Default)]
pub(crate) struct PendingUpdate(Mutex<Option<Update>>);

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplicationInfo {
    version: &'static str,
    os: String,
    architecture: &'static str,
    updater_enabled: bool,
    update_message: &'static str,
}

#[derive(Serialize, TS)]
pub(crate) struct UpdateInfo {
    version: String,
    notes: String,
}

#[tauri::command]
pub(crate) fn application_info() -> ApplicationInfo {
    let info = os_info::get();
    let (updater_enabled, update_message) = match update_key() {
        Ok(_) => (true, "Updates are verified with the embedded signing key."),
        Err(message) => (false, message),
    };
    ApplicationInfo {
        version: env!("CARGO_PKG_VERSION"),
        os: format!("{} {}", info.os_type(), info.version()),
        architecture: std::env::consts::ARCH,
        updater_enabled,
        update_message,
    }
}

fn update_key() -> std::result::Result<&'static str, &'static str> {
    if cfg!(dev) {
        return Err("Application updates are available in signed release builds.");
    }
    if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_none() {
        return Err("Use your package manager or install the latest RPM from GitHub Releases.");
    }
    UPDATER_KEY
        .filter(|key| !key.trim().is_empty())
        .ok_or("This build has no updater signing key. Use GitHub Releases for updates.")
}

#[tauri::command]
pub(crate) async fn check_application_update(app: AppHandle) -> Result<Option<UpdateInfo>> {
    let key = update_key().map_err(invalid)?;
    let pending = app.state::<PendingUpdate>();
    *pending
        .0
        .lock()
        .map_err(|_| invalid("The updater is unavailable."))? = None;
    let update = app
        .updater_builder()
        .pubkey(key)
        .endpoints(vec![Url::parse(UPDATE_FEED)?])
        .map_err(update_error)?
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(update_error)?
        .check()
        .await
        .map_err(update_error)?;
    let info = update.as_ref().map(|update| UpdateInfo {
        version: update.version.clone(),
        notes: update.body.clone().unwrap_or_default(),
    });
    *pending
        .0
        .lock()
        .map_err(|_| invalid("The updater is unavailable."))? = update;
    Ok(info)
}

#[tauri::command]
pub(crate) async fn install_application_update(app: AppHandle, version: String) -> Result<()> {
    update_key().map_err(invalid)?;
    let manager = app.state::<DownloadManager>();
    let _maintenance = manager.reserve_maintenance()?;
    let pending = app.state::<PendingUpdate>();
    let update = pending
        .0
        .lock()
        .map_err(|_| invalid("The updater is unavailable."))?
        .take()
        .filter(|update| update.version == version)
        .ok_or_else(|| invalid("Check for the update again before installing."))?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(update_error)?;
    // Linux AppImage updates require a restart. Windows installs after exiting.
    app.restart();
}

#[tauri::command]
pub(crate) async fn export_diagnostics(app: AppHandle, path: PathBuf) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || {
        if !path.is_absolute()
            || path
                .extension()
                .is_none_or(|ext| !ext.eq_ignore_ascii_case("json"))
        {
            return Err(invalid("Select a JSON file for the diagnostics export."));
        }
        let storage = app.state::<Arc<Storage>>();
        let manager = app.state::<DownloadManager>();
        let backends = manager
            .backend_info(&storage.settings()?, false)?
            .into_iter()
            .map(|backend| {
                serde_json::json!({
                    "name": backend.backend.name(),
                    "managed": backend.managed,
                    "state": backend.state,
                    "version": backend.version,
                })
            })
            .collect::<Vec<_>>();
        let jobs = storage
            .jobs()?
            .into_iter()
            .rev()
            .take(100)
            .map(|job| {
                let engine = match job.provider.as_str() {
                    "steamcmd" => Some(BackendChoice::SteamCmd),
                    "depotDownloader" => Some(BackendChoice::DepotDownloader),
                    _ => None,
                };
                serde_json::json!({
                    "createdAt": job.created_at,
                    "updatedAt": job.updated_at,
                    "requestedBackend": job.requested_backend,
                    "engine": engine,
                    "status": job.status,
                    "failureKind": job.failure_kind,
                })
            })
            .collect::<Vec<_>>();
        let data = serde_json::json!({
            "format": 1,
            "application": application_info(),
            "backends": backends,
            "downloadActivity": jobs,
        });
        // Generate a structured export without raw CLI output, paths, item IDs,
        // account identifiers, free-form errors or authentication material.
        let parent = path
            .parent()
            .ok_or_else(|| invalid("Choose a file in an existing directory."))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temporary, &data)?;
        writeln!(temporary)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    })
    .await
    .map_err(|_| invalid("The diagnostics export did not finish."))?
}

#[tauri::command]
pub(crate) fn open_application_releases() -> Result<()> {
    crate::download_commands::open_target(std::ffi::OsStr::new(
        "https://github.com/NotYetFound/Workshop-Downloader/releases",
    ))
}

fn invalid(message: &str) -> AppError {
    AppError::Invalid(message.into())
}

fn update_error(error: tauri_plugin_updater::Error) -> AppError {
    invalid(&format!("Could not update Workshop Downloader. {error}"))
}

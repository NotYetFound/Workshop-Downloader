mod aux_windows;
mod browser;
mod destination;
mod download_commands;
pub mod downloads;
mod maintenance;
mod settings;
pub mod steam;
pub mod storage;
mod workshop;

#[cfg(test)]
use std::path::PathBuf;
use std::{fs, sync::Arc};

use browser::{Browser, BrowserAction, BrowserBounds, BrowserState};
use downloads::{DownloadJob, DownloadManager};
use serde::{Deserialize, Serialize, Serializer};
use settings::Settings;
use steam::{SteamClient, WorkshopItem};
use storage::Storage;
use tauri::{AppHandle, Manager, State, WebviewWindowBuilder};
use ts_rs::TS;

type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Invalid(String),
    #[error("The Workshop browser is unavailable.")]
    BrowserUnavailable,
    #[error("Could not reach Steam or read its response. {0}")]
    Steam(#[from] reqwest::Error),
    #[error("Could not access the application database. {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Could not read application data. {0}")]
    Data(#[from] serde_json::Error),
    #[error("Could not access a local directory. {0}")]
    Io(#[from] std::io::Error),
    #[error("The desktop runtime reported an error. {0}")]
    Desktop(#[from] tauri::Error),
    #[error("The Steam address is invalid. {0}")]
    Url(#[from] url::ParseError),
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
enum Destination {
    Url(String),
    Game(u32),
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
struct ItemInspection {
    item: WorkshopItem,
    game_name: String,
    destination: String,
}

#[tauri::command]
fn get_settings(storage: State<'_, Arc<Storage>>) -> Result<Settings> {
    storage.settings()
}

#[tauri::command]
fn save_settings(storage: State<'_, Arc<Storage>>, settings: Settings) -> Result<()> {
    storage.save_settings(&settings)?;
    downloads::transcript::set_verbose(settings.verbose_diagnostics);
    Ok(())
}

#[tauri::command]
fn list_downloads(storage: State<'_, Arc<Storage>>) -> Result<Vec<DownloadJob>> {
    storage.jobs()
}

#[tauri::command]
fn browser_state(app: AppHandle, browser: State<'_, Browser>) -> Result<BrowserState> {
    browser.state(&app)
}

#[tauri::command]
fn take_workshop_preview(browser: State<'_, Browser>) -> Result<Option<Vec<String>>> {
    browser.take_preview_request()
}

#[tauri::command]
fn copy_workshop_url(app: AppHandle) -> Result<()> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let url = app
        .state::<Browser>()
        .state(&app)?
        .url
        .ok_or(AppError::BrowserUnavailable)?;
    app.clipboard()
        .write_text(url)
        .map_err(|error| AppError::Invalid(error.to_string()))
}

async fn on_ui_thread<T: Send + 'static>(
    app: AppHandle,
    task: impl FnOnce(&AppHandle) -> Result<T> + Send + 'static,
) -> Result<T> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = sender.send(task(&handle));
    })?;
    receiver.await.map_err(|_| AppError::BrowserUnavailable)?
}

#[tauri::command]
async fn browser_action(app: AppHandle, action: BrowserAction) -> Result<()> {
    on_ui_thread(app, move |app| app.state::<Browser>().action(app, action)).await
}

#[tauri::command]
async fn navigate_workshop(app: AppHandle, destination: Destination) -> Result<()> {
    let url = match destination {
        Destination::Url(value) => steam::parse_address(&value)?,
        Destination::Game(id) => steam::game_url(id)?,
    };
    on_ui_thread(app, move |app| app.state::<Browser>().navigate(app, url)).await
}

#[tauri::command]
async fn set_browser_slot(app: AppHandle, bounds: Option<BrowserBounds>) -> Result<()> {
    on_ui_thread(app, move |app| app.state::<Browser>().set_slot(app, bounds)).await
}

#[tauri::command]
async fn inspect_item(
    app: AppHandle,
    steam: State<'_, SteamClient>,
    address: String,
) -> Result<ItemInspection> {
    let id = steam::item_id(&steam::parse_address(&address)?)
        .ok_or_else(|| AppError::Invalid("Open a Steam Workshop item to inspect it.".into()))?;
    let item = steam.resolve_item(&id).await?;
    tauri::async_runtime::spawn_blocking(move || {
        let settings = app.state::<Arc<Storage>>().settings()?;
        let game_name = settings.game(item.app_id).map_or_else(
            || format!("Steam App {}", item.app_id),
            |game| game.name.clone(),
        );
        let path = destination::plan(&settings, &item)?;
        let destination = path.into_os_string().into_string().map_err(|_| {
            AppError::Invalid("The destination path could not be represented as text.".into())
        })?;
        Ok(ItemInspection {
            item,
            game_name,
            destination,
        })
    })
    .await
    .map_err(|_| AppError::Invalid("Could not calculate the download destination.".into()))?
}

pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(maintenance::PendingUpdate::default())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            list_downloads,
            browser_state,
            copy_workshop_url,
            take_workshop_preview,
            browser_action,
            navigate_workshop,
            set_browser_slot,
            inspect_item,
            workshop::commands::workshop_is_collection,
            workshop::commands::preview_workshop,
            workshop::commands::queue_workshop_batch,
            workshop::commands::list_batches,
            workshop::commands::cancel_batch,
            workshop::commands::retry_batch,
            workshop::commands::content_states,
            workshop::commands::scan_external,
            workshop::commands::list_external_items,
            workshop::commands::open_external_folder,
            download_commands::queue_download,
            download_commands::retry_download,
            download_commands::cancel_download,
            download_commands::remove_download,
            download_commands::clear_download_history,
            download_commands::download_logs,
            download_commands::download_activity,
            download_commands::terminal_sessions,
            aux_windows::open_download_logs_window,
            aux_windows::sync_download_auth_window,
            download_commands::download_prompt,
            download_commands::authenticate_download,
            download_commands::item_download_state,
            download_commands::sync_downloaded_badges,
            download_commands::open_download_folder,
            download_commands::download_backends,
            download_commands::open_backend_source,
            download_commands::backend_release,
            download_commands::install_backend,
            download_commands::restore_backend,
            download_commands::clear_backend_session,
            maintenance::application_info,
            maintenance::check_application_update,
            maintenance::install_application_update,
            maintenance::open_application_releases,
            maintenance::export_diagnostics,
        ])
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                for label in [aux_windows::AUTH_LABEL, aux_windows::LOGS_LABEL] {
                    if let Some(aux) = window.app_handle().get_webview_window(label) {
                        let _ = aux.destroy();
                    }
                }
            }
        })
        .setup(|app| {
            let root = app.path().app_local_data_dir()?;
            let profile = root.join("steam-profile");
            fs::create_dir_all(&profile)?;
            let downloads = app
                .path()
                .download_dir()
                .or_else(|_| app.path().home_dir().map(|home| home.join("Downloads")))?;
            let storage = Arc::new(Storage::open(
                &root.join("workshop.sqlite3"),
                &Settings::new(downloads)?,
            )?);
            app.manage(DownloadManager::new(
                storage.clone(),
                root.join("downloads-engine"),
            )?);
            downloads::transcript::set_verbose(storage.settings()?.verbose_diagnostics);
            app.manage(storage);
            app.manage(SteamClient::new()?);
            app.manage(Browser::new(profile));
            let config = app
                .config()
                .app
                .windows
                .iter()
                .find(|window| window.label == "main")
                .ok_or_else(|| {
                    AppError::Invalid("The desktop window configuration is missing.".into())
                })?;
            let dev_origin = app.config().build.dev_url.as_ref().map(|url| url.origin());
            WebviewWindowBuilder::from_config(app, config)?
                .on_navigation(move |url| {
                    (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"))
                        || (cfg!(dev) && dev_origin.as_ref() == Some(&url.origin()))
                })
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())?
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                app.state::<DownloadManager>().shutdown();
            }
        });
    Ok(())
}

#[test]
fn export_frontend_contracts() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/lib/contracts");
    let config = ts_rs::Config::default().with_out_dir(directory);
    Settings::export_all(&config).unwrap();
    BrowserAction::export_all(&config).unwrap();
    BrowserBounds::export_all(&config).unwrap();
    BrowserState::export_all(&config).unwrap();
    Destination::export_all(&config).unwrap();
    ItemInspection::export_all(&config).unwrap();
    DownloadJob::export_all(&config).unwrap();
    downloads::QueueOptions::export_all(&config).unwrap();
    downloads::DownloadLog::export_all(&config).unwrap();
    downloads::DownloadActivity::export_all(&config).unwrap();
    downloads::transcript::TerminalSession::export_all(&config).unwrap();
    downloads::AuthPrompt::export_all(&config).unwrap();
    downloads::ItemDownloadState::export_all(&config).unwrap();
    workshop::WorkshopPreview::export_all(&config).unwrap();
    workshop::BatchProgress::export_all(&config).unwrap();
    workshop::external::ScanReport::export_all(&config).unwrap();
    workshop::external::ExternalSummary::export_all(&config).unwrap();
    downloads::BackendInfo::export_all(&config).unwrap();
    downloads::BackendRelease::export_all(&config).unwrap();
    maintenance::ApplicationInfo::export_all(&config).unwrap();
    maintenance::UpdateInfo::export_all(&config).unwrap();
}

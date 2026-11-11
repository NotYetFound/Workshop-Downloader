use std::{ffi::OsStr, sync::Arc};

use tauri::{AppHandle, Manager, State};
use zeroize::Zeroizing;

use crate::{
    AppError, Result,
    browser::Browser,
    downloads::{
        AuthPrompt, BackendChoice, BackendInfo, DownloadJob, DownloadLog, DownloadManager,
        ItemDownloadState, QueueOptions, official_source,
    },
    on_ui_thread,
    settings::Settings,
    steam::{self, SteamClient},
    storage::Storage,
};

async fn background<T: Send + 'static>(
    app: AppHandle,
    task: impl FnOnce(&AppHandle) -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(move || task(&app))
        .await
        .map_err(|_| AppError::Invalid("The download operation did not finish.".into()))?
}

#[tauri::command]
pub(crate) async fn queue_download(
    app: AppHandle,
    client: State<'_, SteamClient>,
    address: String,
    options: QueueOptions,
    preview_id: Option<String>,
) -> Result<DownloadJob> {
    let id = steam::item_id(&steam::parse_address(&address)?).ok_or_else(|| {
        AppError::Invalid("Open an individual Workshop item to download it.".into())
    })?;
    if let Some(preview_id) = preview_id {
        let preview = app.state::<Arc<Storage>>().preview(&preview_id)?;
        let eligible = preview.collection_id.is_none()
            && matches!(preview.items.as_slice(), [entry] if entry.id == id && entry.item.is_some() && entry.unavailable.is_none());
        if !eligible {
            return Err(AppError::Invalid(
                "Open the Workshop item again to download it.".into(),
            ));
        }
    } else {
        let requested = id.clone();
        on_ui_thread(app.clone(), move |app| {
            if app.state::<Browser>().state(app)?.item_id.as_deref() != Some(&requested) {
                return Err(AppError::Invalid(
                    "The Workshop page changed. Open the item again to download it.".into(),
                ));
            }
            Ok(())
        })
        .await?;
    }
    let metadata = client.resolve_metadata(&id, true).await?;
    let mut job = DownloadJob::new(metadata.item, &options, metadata.updated_at);
    job.expected_bytes = metadata.size_bytes;
    background(app, move |app| {
        app.state::<DownloadManager>().enqueue(job, options)
    })
    .await
}

#[tauri::command]
pub(crate) async fn retry_download(
    app: AppHandle,
    id: String,
    options: QueueOptions,
) -> Result<()> {
    background(app, move |app| {
        app.state::<DownloadManager>().retry(&id, options)
    })
    .await
}

#[tauri::command]
pub(crate) fn cancel_download(manager: State<'_, DownloadManager>, id: String) -> Result<()> {
    manager.cancel(&id)
}

#[tauri::command]
pub(crate) fn remove_download(manager: State<'_, DownloadManager>, id: String) -> Result<()> {
    manager.remove(&id)
}

#[tauri::command]
pub(crate) fn download_logs(
    storage: State<'_, Arc<Storage>>,
    id: String,
) -> Result<Vec<DownloadLog>> {
    storage.logs(&id)
}

#[tauri::command]
pub(crate) fn download_prompt(manager: State<'_, DownloadManager>) -> Result<Option<AuthPrompt>> {
    manager.prompt()
}

#[tauri::command]
pub(crate) fn authenticate_download(
    manager: State<'_, DownloadManager>,
    id: String,
    response: String,
) -> Result<()> {
    manager.authenticate(&id, Zeroizing::new(response))
}

#[tauri::command]
pub(crate) async fn item_download_state(app: AppHandle, id: String) -> Result<ItemDownloadState> {
    background(app, move |app| {
        app.state::<DownloadManager>().state_for(&id)
    })
    .await
}

#[tauri::command]
pub(crate) async fn sync_downloaded_badges(app: AppHandle) -> Result<()> {
    let ids = background(app.clone(), |app| {
        app.state::<DownloadManager>().content_states()
    })
    .await?;
    on_ui_thread(app, move |app| {
        app.state::<Browser>().sync_badges(app, &ids)
    })
    .await
}

#[tauri::command]
pub(crate) async fn open_download_folder(
    app: AppHandle,
    id: String,
    workshop_item: bool,
) -> Result<()> {
    background(app, move |app| {
        let manager = app.state::<DownloadManager>();
        let path = if workshop_item {
            manager.open_item_path(&id)?
        } else {
            manager.open_job_path(&id)?
        };
        open_target(path.as_os_str())
    })
    .await
}

#[tauri::command]
pub(crate) async fn download_backends(
    app: AppHandle,
    settings: Settings,
    check: bool,
) -> Result<Vec<BackendInfo>> {
    settings.validate()?;
    background(app, move |app| {
        app.state::<DownloadManager>()
            .backend_info(&settings, check)
    })
    .await
}

#[tauri::command]
pub(crate) fn open_backend_source(backend: BackendChoice) -> Result<()> {
    let url =
        official_source(backend).ok_or_else(|| AppError::Invalid("Select a backend.".into()))?;
    open_target(OsStr::new(url))
}

#[cfg(target_os = "linux")]
pub(crate) fn open_target(target: &OsStr) -> Result<()> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(windows)]
pub(crate) fn open_target(target: &OsStr) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

    let target: Vec<_> = target.encode_wide().chain([0]).collect();
    let open: Vec<_> = "open".encode_utf16().chain([0]).collect();
    // The target is an absolute verified install path or a fixed official URL.
    let status = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            open.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if status as isize <= 32 {
        return Err(AppError::Invalid(
            "Windows could not open the destination.".into(),
        ));
    }
    Ok(())
}

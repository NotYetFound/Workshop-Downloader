use std::{ffi::OsStr, sync::Arc};

use tauri::{AppHandle, Manager, State};
use zeroize::Zeroizing;

use crate::{
    AppError, Result,
    browser::Browser,
    downloads::{
        AuthPrompt, BackendChoice, BackendInfo, BackendRelease, DownloadJob, DownloadLog,
        DownloadManager, ItemDownloadState, QueueOptions, official_source,
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
    enqueue_item(app, &client, &id, options).await
}

#[tauri::command]
pub(crate) async fn queue_download_by_id(
    app: AppHandle,
    client: State<'_, SteamClient>,
    id: String,
    options: QueueOptions,
) -> Result<DownloadJob> {
    let id = steam::validate_id(id.trim())?;
    enqueue_item(app, &client, &id, options).await
}

async fn enqueue_item(
    app: AppHandle,
    client: &SteamClient,
    id: &str,
    options: QueueOptions,
) -> Result<DownloadJob> {
    let metadata = client.resolve_metadata(id, true).await?;
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
pub(crate) fn clear_download_history(manager: State<'_, DownloadManager>) -> Result<()> {
    manager.clear_history()
}

#[tauri::command]
pub(crate) fn download_activity(
    storage: State<'_, Arc<Storage>>,
) -> Result<Vec<crate::downloads::DownloadActivity>> {
    storage.download_activity()
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
pub(crate) fn executable_locations() -> Result<Vec<String>> {
    #[cfg(target_os = "linux")]
    {
        let mounts = std::fs::read("/proc/self/mountinfo")?;
        Ok(mounted_locations(&String::from_utf8_lossy(&mounts)))
    }
    #[cfg(not(target_os = "linux"))]
    Ok(Vec::new())
}

#[cfg(target_os = "linux")]
fn mounted_locations(mounts: &str) -> Vec<String> {
    let mut locations = std::collections::BTreeSet::new();
    for line in mounts.lines() {
        let Some((mount, filesystem)) = line.split_once(" - ") else {
            continue;
        };
        let Some(path) = mount.split_ascii_whitespace().nth(4) else {
            continue;
        };
        let mut filesystem = filesystem.split_ascii_whitespace();
        let kind = filesystem.next().unwrap_or_default();
        let source = filesystem.next().unwrap_or_default();
        let storage = source.starts_with("/dev/") && !source.starts_with("/dev/loop")
            || matches!(kind, "nfs" | "nfs4" | "cifs" | "smb3" | "fuse.sshfs" | "zfs");
        let media = ["/mnt/", "/media/", "/run/media/"]
            .iter()
            .any(|prefix| path.starts_with(prefix));
        if (storage || media) && path != "/" && !path.starts_with("/boot") {
            locations.insert(
                path.replace(r"\040", " ")
                    .replace(r"\011", "\t")
                    .replace(r"\012", "\n")
                    .replace(r"\134", "\\"),
            );
        }
    }
    locations.into_iter().collect()
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn executable_locations_include_mounted_storage() {
    let mounts = r"1 0 8:1 / / rw - ext4 /dev/sda1 rw
2 1 0:2 / /proc rw - proc proc rw
3 1 8:2 / /run/media/user/My\040Drive rw - exfat /dev/sdb1 rw
4 1 0:4 / /storage/remote rw - nfs host:/share rw
5 1 7:0 / /tmp/.mount_workshop rw - squashfs /dev/loop0 ro";
    assert_eq!(
        mounted_locations(mounts),
        ["/run/media/user/My Drive", "/storage/remote"]
    );
}

#[tauri::command]
pub(crate) fn backend_notices(manager: State<'_, DownloadManager>) -> Vec<String> {
    manager.take_backend_notices()
}

#[tauri::command]
pub(crate) fn backend_issues(
    storage: State<'_, Arc<Storage>>,
) -> Result<std::collections::BTreeMap<String, String>> {
    storage.backend_issues()
}

#[tauri::command]
pub(crate) fn clear_backend_issue(
    storage: State<'_, Arc<Storage>>,
    backend: BackendChoice,
) -> Result<()> {
    storage.clear_backend_issue(backend.id())
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
pub(crate) async fn backend_release(
    app: AppHandle,
    backend: BackendChoice,
) -> Result<BackendRelease> {
    background(app, move |app| {
        app.state::<DownloadManager>().backend_release(backend)
    })
    .await
}

#[tauri::command]
pub(crate) async fn install_backend(
    app: AppHandle,
    backend: BackendChoice,
    source: String,
) -> Result<()> {
    background(app, move |app| {
        app.state::<DownloadManager>()
            .install_backend(backend, &source)
    })
    .await
}

#[tauri::command]
pub(crate) async fn restore_backend(app: AppHandle, backend: BackendChoice) -> Result<()> {
    background(app, move |app| {
        app.state::<DownloadManager>().restore_backend(backend)
    })
    .await
}

#[tauri::command]
pub(crate) async fn clear_backend_session(app: AppHandle, backend: BackendChoice) -> Result<()> {
    background(app, move |app| {
        app.state::<DownloadManager>()
            .clear_backend_session(backend)
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

#[tauri::command]
pub(crate) fn terminal_sessions() -> Vec<crate::downloads::transcript::TerminalSession> {
    crate::downloads::transcript::sessions()
}

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::{
    Result,
    downloads::{DownloadManager, PromptKind},
};

pub(crate) const LOGS_LABEL: &str = "download-logs";
pub(crate) const AUTH_LABEL: &str = "download-auth";

#[tauri::command]
pub(crate) async fn sync_download_auth_window(app: AppHandle) -> Result<()> {
    let prompt = app.state::<DownloadManager>().prompt()?;
    if let Some(window) = app.get_webview_window(AUTH_LABEL) {
        if prompt.is_none() {
            window.destroy()?;
        }
        return Ok(());
    }
    let Some(prompt) = prompt else {
        return Ok(());
    };
    let window = local_window(&app, AUTH_LABEL)
        .title("Steam sign-in — Workshop Downloader")
        .inner_size(
            420.0,
            if prompt.kind == PromptKind::Qr {
                395.0
            } else {
                260.0
            },
        )
        .min_inner_size(360.0, 240.0)
        .build()?;
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            let manager = handle.state::<DownloadManager>();
            if manager
                .prompt()
                .ok()
                .flatten()
                .is_some_and(|current| current.job_id == prompt.job_id)
                && manager.cancel(&prompt.job_id).is_ok()
            {
                api.prevent_close();
                if let Some(auth) = handle.get_webview_window(AUTH_LABEL) {
                    let _ = auth.hide();
                }
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub(crate) async fn open_download_logs_window(app: AppHandle) -> Result<()> {
    if let Some(window) = app.get_webview_window(LOGS_LABEL) {
        window.unminimize()?;
        window.set_focus()?;
    } else {
        local_window(&app, LOGS_LABEL)
            .title("Debug — Workshop Downloader")
            .inner_size(760.0, 500.0)
            .min_inner_size(420.0, 280.0)
            .build()?;
    }
    Ok(())
}

fn local_window<'a>(
    app: &'a AppHandle,
    label: &str,
) -> WebviewWindowBuilder<'a, tauri::Wry, AppHandle> {
    let dev_origin = app.config().build.dev_url.as_ref().map(|url| url.origin());
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .center()
        .theme(Some(tauri::Theme::Dark))
        .on_navigation(move |url| {
            (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"))
                || (cfg!(dev) && dev_origin.as_ref() == Some(&url.origin()))
        })
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
}

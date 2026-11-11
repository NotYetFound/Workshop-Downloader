#[cfg(target_os = "linux")]
mod gtk_slot;

use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::Arc;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, Rect, Webview, WebviewUrl,
    webview::{NewWindowResponse, PermissionResponse, WebviewBuilder},
};
use ts_rs::TS;
use url::Url;

use crate::{AppError, Result, steam};

const STEAM_VIEW: &str = "steam-docked";
const ENHANCEMENTS: &str = include_str!("injection/workshop.js");
const GALLERY: &str = include_str!("injection/workshop_gallery.js");

#[derive(Clone, Copy, Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BrowserAction {
    Back,
    Forward,
    Refresh,
    Home,
}

#[derive(Clone, Copy, Debug, Deserialize, TS)]
pub struct BrowserBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl BrowserBounds {
    fn valid(self) -> bool {
        (0.0..=16_384.0).contains(&self.x)
            && (0.0..=16_384.0).contains(&self.y)
            && (1.0..=16_384.0).contains(&self.width)
            && (1.0..=16_384.0).contains(&self.height)
    }

    fn rect(self) -> Rect {
        Rect {
            position: LogicalPosition::new(self.x, self.y).into(),
            size: LogicalSize::new(self.width, self.height).into(),
        }
    }
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserState {
    pub url: Option<String>,
    pub item_id: Option<String>,
    pub app_id: Option<u32>,
    pub open: bool,
}

pub struct Browser {
    profile: PathBuf,
    preview_request: Mutex<Option<Vec<String>>>,
    #[cfg(target_os = "linux")]
    slot: Arc<Mutex<Option<BrowserBounds>>>,
}

impl Browser {
    pub fn new(profile: PathBuf) -> Self {
        Self {
            profile,
            preview_request: Mutex::new(None),
            #[cfg(target_os = "linux")]
            slot: Arc::new(Mutex::new(None)),
        }
    }

    pub fn take_preview_request(&self) -> Result<Option<Vec<String>>> {
        Ok(self
            .preview_request
            .lock()
            .map_err(|_| AppError::BrowserUnavailable)?
            .take())
    }
    pub fn state(&self, app: &AppHandle) -> Result<BrowserState> {
        let view = app.get_webview(STEAM_VIEW);
        let mut url = view
            .as_ref()
            .map(Webview::url)
            .transpose()?
            .filter(steam::is_browser_url);
        if let Some(current) = url.as_ref()
            && let Some(ids) = preview_request(current)
        {
            // Steam's SPA router can commit a same-document URL without invoking
            // WebKit's navigation-policy callback. Capture that route here too.
            store_preview_request(app, ids);
            let mut cleaned = current.clone();
            let query = cleaned
                .query_pairs()
                .filter(|(key, _)| key != "workshopdesk_preview")
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect::<Vec<_>>();
            cleaned.set_query(None);
            if !query.is_empty() {
                cleaned.query_pairs_mut().extend_pairs(query);
            }
            if let Some(view) = view.as_ref() {
                view.eval(format!(
                    "window.history.replaceState(window.history.state, '', {});",
                    serde_json::to_string(cleaned.as_str())?
                ))?;
            }
            url = Some(cleaned);
        }
        Ok(BrowserState {
            item_id: url.as_ref().and_then(steam::item_id),
            app_id: url.as_ref().and_then(steam::app_id),
            url: url.map(|url| url.to_string()),
            open: view.is_some(),
        })
    }

    /// Runs on the native UI thread. The first visible slot starts Steam Home.
    pub fn set_slot(&self, app: &AppHandle, slot: Option<BrowserBounds>) -> Result<()> {
        if slot.is_some_and(|bounds| !bounds.valid()) {
            return Err(AppError::Invalid("Invalid browser view dimensions.".into()));
        }
        #[cfg(target_os = "linux")]
        {
            *self.slot.lock().map_err(|_| AppError::BrowserUnavailable)? = slot;
        }
        if let Some(bounds) = slot {
            let view = match app.get_webview(STEAM_VIEW) {
                Some(view) => view,
                None => self.open(app, Url::parse(steam::WORKSHOP_HOME)?, bounds)?,
            };
            #[cfg(target_os = "linux")]
            gtk_slot::refresh(&view)?;
            #[cfg(not(target_os = "linux"))]
            view.set_bounds(bounds.rect())?;
            view.show()?;
        } else if let Some(view) = app.get_webview(STEAM_VIEW) {
            view.hide()?;
        }
        Ok(())
    }

    pub fn navigate(&self, app: &AppHandle, url: Url) -> Result<()> {
        Self::view(app)?.navigate(url)?;
        Ok(())
    }

    pub fn action(&self, app: &AppHandle, action: BrowserAction) -> Result<()> {
        let view = Self::view(app)?;
        match action {
            BrowserAction::Back | BrowserAction::Forward => {
                #[cfg(target_os = "linux")]
                gtk_slot::history(&view, action)?;
                #[cfg(not(target_os = "linux"))]
                view.eval(match action {
                    BrowserAction::Back => "window.history.back()",
                    _ => "window.history.forward()",
                })?;
            }
            BrowserAction::Refresh => view.reload()?,
            BrowserAction::Home => view.navigate(Url::parse(steam::WORKSHOP_HOME)?)?,
        }
        Ok(())
    }

    pub fn sync_badges(
        &self,
        app: &AppHandle,
        ids: &std::collections::HashMap<String, crate::downloads::ItemDownloadState>,
    ) -> Result<()> {
        if let Some(view) = app.get_webview(STEAM_VIEW)
            && let Ok(url) = view.url()
            && steam::is_community_url(&url)
            && (url.path().contains("/workshop") || steam::item_id(&url).is_some())
        {
            view.eval(format!(
                "window.dispatchEvent(new CustomEvent('workshop-desk-downloaded', {{ detail: {} }}));",
                serde_json::to_string(&ids.iter().map(|(id,state)|(id,&state.status)).collect::<std::collections::HashMap<_,_>>())?,
            ))?;
        }
        Ok(())
    }

    fn view(app: &AppHandle) -> Result<Webview> {
        app.get_webview(STEAM_VIEW)
            .ok_or(AppError::BrowserUnavailable)
    }

    fn open(&self, app: &AppHandle, url: Url, bounds: BrowserBounds) -> Result<Webview> {
        let window = app.get_window("main").ok_or(AppError::BrowserUnavailable)?;
        #[cfg(target_os = "linux")]
        let container = gtk_slot::Container::new(&window)?;

        let popup_app = app.clone();
        let navigation_app = app.clone();
        let builder = WebviewBuilder::new(STEAM_VIEW, WebviewUrl::External(url))
            .data_directory(self.profile.clone())
            .initialization_script(ENHANCEMENTS)
            .initialization_script(GALLERY)
            .on_navigation(move |url| {
                if let Some(ids) = preview_request(url) {
                    store_preview_request(&navigation_app, ids);
                    return false;
                }
                #[cfg(target_os = "linux")]
                if gtk_slot::is_youtube_embed(url) {
                    return true;
                }
                steam::is_browser_url(url)
            })
            .on_new_window(move |url, _| redirect_popup(&popup_app, url))
            .on_download(|_, _| false)
            .on_permission_request(|_, _| PermissionResponse::Deny)
            .background_color(tauri::webview::Color(18, 18, 20, 255))
            .focused(false)
            .devtools(false);
        let rect = bounds.rect();
        let view = window.add_child(builder, rect.position, rect.size)?;
        #[cfg(target_os = "linux")]
        if let Err(error) = container.attach(self.slot.clone()) {
            let _ = view.close();
            return Err(error);
        }
        Ok(view)
    }
}

fn preview_request(url: &Url) -> Option<Vec<String>> {
    if !steam::is_community_url(url) {
        return None;
    }
    let mut queries = url
        .query_pairs()
        .filter(|(key, _)| key == "workshopdesk_preview");
    let (_, value) = queries.next()?;
    if queries.next().is_some() || value.len() > 42_000 {
        return Some(vec![]);
    }
    let ids = value
        .split(',')
        .map(steam::validate_id)
        .collect::<Result<Vec<_>>>()
        .ok()?;
    if ids.is_empty() || ids.len() > 2000 {
        return Some(vec![]);
    }
    // This bridge only opens a trusted preview. It never starts a download.
    Some(ids)
}

fn store_preview_request(app: &AppHandle, ids: Vec<String>) {
    if let Ok(mut pending) = app.state::<Browser>().preview_request.lock() {
        // The shell polls this bounded one-shot request; no remote code can queue jobs.
        *pending = Some(ids);
    }
}

fn redirect_popup(app: &AppHandle, url: Url) -> NewWindowResponse<tauri::Wry> {
    if let Some(ids) = preview_request(&url) {
        store_preview_request(app, ids);
    } else if steam::is_browser_url(&url) {
        let app = app.clone();
        // Navigate after the native new-window callback has returned.
        tauri::async_runtime::spawn(async move {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Ok(view) = Browser::view(&handle) {
                    let _ = view.navigate(url);
                }
            });
        });
    }
    NewWindowResponse::Deny
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_controls_can_only_request_bounded_numeric_previews() {
        let url = Url::parse(
            "https://steamcommunity.com/workshop/browse/?appid=1&workshopdesk_preview=123%2C456",
        )
        .unwrap();
        assert_eq!(
            preview_request(&url),
            Some(vec!["123".into(), "456".into()])
        );
        for url in [
            "https://example.com/?workshopdesk_preview=123",
            "https://steamcommunity.com/?workshopdesk_preview=0",
            "https://steamcommunity.com/?workshopdesk_preview=../data",
        ] {
            assert!(preview_request(&Url::parse(url).unwrap()).is_none());
        }
        assert_eq!(
            preview_request(
                &Url::parse(
                    "https://steamcommunity.com/?workshopdesk_preview=1&workshopdesk_preview=2"
                )
                .unwrap()
            ),
            Some(vec![])
        );
    }
}

use std::{
    cell::Cell,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gtk::{OverlaySignals, gdk::Rectangle, prelude::*};
use tauri::{Webview, Window};
use url::Url;
use webkit2gtk::{
    HardwareAccelerationPolicy, LoadEvent, PolicyDecisionExt, PolicyDecisionType,
    ResponsePolicyDecision, ResponsePolicyDecisionExt, SettingsExt, URIRequestExt, WebView,
    WebViewExt,
};

use super::{BrowserAction, BrowserBounds};
use crate::{AppError, Result};

/// Wry adds Linux child views to Tauri's default GtkBox, where the box allocates
/// the children as rows and ignores their requested position. Put both widgets
/// in a GtkOverlay and let GTK allocate the Steam widget to the shell's slot.
pub(super) struct Container {
    root: gtk::Box,
    shell: WebView,
}

impl Container {
    pub(super) fn new(window: &Window) -> Result<Self> {
        let root = window.default_vbox()?;
        let shell = root
            .children()
            .into_iter()
            .find_map(|child| child.downcast::<WebView>().ok())
            .or_else(|| {
                // A failed child creation can be retried after the overlay exists.
                root.children()
                    .into_iter()
                    .find_map(|child| child.downcast::<gtk::Overlay>().ok())?
                    .child()?
                    .downcast::<WebView>()
                    .ok()
            })
            .ok_or_else(layout_error)?;
        Ok(Self { root, shell })
    }

    pub(super) fn attach(
        self,
        slot: Arc<Mutex<Option<BrowserBounds>>>,
        interrupted: Arc<AtomicBool>,
    ) -> Result<()> {
        let steam = self
            .root
            .children()
            .into_iter()
            .filter_map(|child| child.downcast::<WebView>().ok())
            .find(|child| *child != self.shell)
            .ok_or_else(layout_error)?;

        let overlay = if let Some(overlay) = self.shell.parent().and_downcast::<gtk::Overlay>() {
            overlay
        } else {
            let overlay = gtk::Overlay::new();
            self.root.remove(&self.shell);
            overlay.add(&self.shell);
            self.root.pack_start(&overlay, true, true, 0);
            overlay.show();
            overlay
        };

        self.root.remove(&steam);
        overlay.add_overlay(&steam);
        steam.connect_decide_policy(|_, decision, kind| {
            if kind != PolicyDecisionType::Response {
                return false;
            }
            let Some(response) = decision.downcast_ref::<ResponsePolicyDecision>() else {
                return false;
            };
            // Wry's navigation callback receives URLs from both frames. Keep
            // the top-level Steam allowlist when the response identifies the frame.
            if response.is_main_frame_main_resource()
                && response
                    .request()
                    .and_then(|request| request.uri())
                    .and_then(|uri| Url::parse(&uri).ok())
                    .is_none_or(|url| !crate::steam::is_browser_url(&url))
            {
                decision.ignore();
                return true;
            }
            false
        });
        let visible_slot = slot.clone();
        recover_web_process(&steam, Some((interrupted, visible_slot)));
        let steam = steam.downgrade();
        overlay.connect_get_child_position(move |overlay, child| {
            if steam.upgrade().as_ref().map(WebView::upcast_ref) != Some(child) {
                return None;
            }
            let bounds = (*slot.lock().ok()?)?;
            let x = bounds.x.round() as i32;
            let y = bounds.y.round() as i32;
            let width =
                ((bounds.x + bounds.width).round() as i32).min(overlay.allocated_width()) - x;
            let height =
                ((bounds.y + bounds.height).round() as i32).min(overlay.allocated_height()) - y;
            Some(Rectangle::new(x, y, width.max(1), height.max(1)))
        });
        Ok(())
    }
}

type BrowserRecovery = (Arc<AtomicBool>, Arc<Mutex<Option<BrowserBounds>>>);

pub(super) fn recover_web_process(view: &WebView, browser: Option<BrowserRecovery>) {
    if let Some((interrupted, slot)) = browser.clone() {
        view.connect_load_changed(move |view, event| {
            if event == LoadEvent::Committed
                && interrupted.swap(false, Ordering::AcqRel)
                && slot.lock().is_ok_and(|slot| slot.is_some())
            {
                view.show();
            }
        });
    }

    let last_retry = Cell::new(None::<Instant>);
    view.connect_web_process_terminated(move |view, reason| {
        eprintln!("WebKit content process stopped: {reason:?}");
        if let Some((interrupted, _)) = &browser {
            interrupted.store(true, Ordering::Release);
            view.hide();
        }

        // A crash can involve graphics compositing. Retry once using software
        // compositing on this view and let repeated failures remain visible.
        if last_retry
            .get()
            .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        last_retry.set(Some(Instant::now()));
        if let Some(settings) = WebViewExt::settings(view) {
            settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never);
        }
        let view = view.downgrade();
        gtk::glib::idle_add_local_once(move || {
            if let Some(view) = view.upgrade() {
                view.reload();
            }
        });
    });
}

pub(super) fn is_youtube_embed(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && matches!(
            url.host_str(),
            Some("www.youtube.com" | "www.youtube-nocookie.com")
        )
        && url.path().strip_prefix("/embed/").is_some_and(|id| {
            id.len() == 11
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

pub(super) fn refresh(view: &Webview) -> Result<()> {
    view.with_webview(|platform| {
        if let Some(parent) = platform.inner().parent() {
            parent.queue_resize();
        }
    })?;
    Ok(())
}

pub(super) fn history(view: &Webview, action: BrowserAction) -> Result<()> {
    view.with_webview(move |platform| {
        let webview = platform.inner();
        match action {
            BrowserAction::Back => webview.go_back(),
            BrowserAction::Forward => webview.go_forward(),
            _ => {}
        }
    })?;
    Ok(())
}

fn layout_error() -> AppError {
    AppError::Invalid("Could not attach Steam to the main window’s GTK layout.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_exception_accepts_only_https_youtube_video_embeds() {
        for (url, allowed) in [
            ("https://www.youtube.com/embed/wpTdk97vjJI?autoplay=1", true),
            ("https://www.youtube-nocookie.com/embed/5suTYz80SVM", true),
            ("https://www.youtube.com/watch?v=wpTdk97vjJI", false),
            ("https://www.youtube.com/embed/wpTdk97vjJI/extra", false),
            (
                "https://www.youtube.com.example.org/embed/wpTdk97vjJI",
                false,
            ),
            ("https://user@www.youtube.com/embed/wpTdk97vjJI", false),
            ("https://www.youtube.com:444/embed/wpTdk97vjJI", false),
            ("http://www.youtube.com/embed/wpTdk97vjJI", false),
        ] {
            let parsed = Url::parse(url).unwrap();
            assert_eq!(is_youtube_embed(&parsed), allowed, "{url}");
            assert!(!crate::steam::is_browser_url(&parsed), "{url}");
        }
    }
}

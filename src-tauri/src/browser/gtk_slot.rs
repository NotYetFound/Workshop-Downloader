use std::sync::{Arc, Mutex};

use gtk::{OverlaySignals, gdk::Rectangle, prelude::*};
use tauri::{Webview, Window};
use webkit2gtk::{WebView, WebViewExt};

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

    pub(super) fn attach(self, slot: Arc<Mutex<Option<BrowserBounds>>>) -> Result<()> {
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

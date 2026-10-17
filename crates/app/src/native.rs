//! AppKit window tweaks GPUI does not expose.

use gpui_kit::Window;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSFloatingWindowLevel, NSView, NSWindowCollectionBehavior};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Keep the window above other apps' windows and on every Space, so the
/// preview follows you around.
pub fn set_floating(window: &Window) {
    // AppKit views are main-thread objects; only touch one on the main
    // thread.
    if MainThreadMarker::new().is_none() {
        return;
    }
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: GPUI hands out the NSView backing this live window, and we are
    // on the main thread (checked above), where AppKit views may be used.
    // The reference does not outlive this call.
    let view: &NSView = unsafe { appkit.ns_view.cast().as_ref() };
    let Some(ns_window) = view.window() else {
        return;
    };
    ns_window.setLevel(NSFloatingWindowLevel);
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    ns_window.setHidesOnDeactivate(false);
}

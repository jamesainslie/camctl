//! camctl: menu bar camera control with a floating live preview.

// GPUI's API is built to be glob-imported, as its own examples do.
#![allow(clippy::wildcard_imports)]

mod assets;
mod away;
mod events;
mod native;
mod preview;
mod status_item;

use futures::StreamExt;
use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;
use objc2::MainThreadMarker;

use crate::preview::{Preview, PreviewEvent};
use crate::status_item::StatusEvent;

/// App-wide state: the preview window and the menu bar icon.
struct Shell {
    preview: Option<(AnyWindowHandle, Entity<Preview>)>,
    status: status_item::StatusItem,
    live_events: Option<Subscription>,
    /// Screen saver running, screen locked.
    saver: bool,
    locked: bool,
    _away: away::AwayWatch,
}

impl Global for Shell {}

fn preview_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(640.), px(420.)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        kind: WindowKind::Normal,
        is_minimizable: false,
        window_min_size: Some(size(px(320.), px(220.))),
        app_id: Some("us.ainslies.camctl".into()),
        ..TitleBar::window_options()
    }
}

fn open_preview(cx: &mut App) -> Option<(AnyWindowHandle, Entity<Preview>)> {
    gpui_kit::open_window(preview_options(cx), cx, |window, cx| {
        native::set_floating(window);
        // Closing hides; the menu bar icon brings it back.
        window.on_window_should_close(cx, |_, cx| {
            cx.hide();
            false
        });
        let shell = cx.global::<Shell>();
        let away = shell.saver || shell.locked;
        cx.new(|cx| Preview::new(away, window, cx))
    })
    .map_err(|e| eprintln!("camctl: could not open the preview window: {e}"))
    .ok()
}

/// Whether the preview window is up and can be seen. A window that is
/// covered, minimized or on a sleeping display counts as not shown, so a
/// click brings it forward instead of hiding it.
fn preview_shown(cx: &mut App) -> bool {
    let Some((handle, _)) = cx.global::<Shell>().preview.clone() else {
        return false;
    };
    handle
        .update(cx, |_, window, _| window.is_visible())
        .unwrap_or(false)
}

fn show_preview(cx: &mut App) {
    let existing = cx.global::<Shell>().preview.clone();
    let alive = existing.as_ref().is_some_and(|(h, _)| {
        h.update(cx, |_, window, _| window.activate_window())
            .is_ok()
    });
    if !alive {
        let opened = open_preview(cx);
        let live = opened.as_ref().map(|(_, preview)| {
            cx.subscribe(preview, |_, event: &PreviewEvent, cx| {
                let PreviewEvent::Live(live) = *event;
                cx.global::<Shell>().status.set_live(live);
            })
        });
        let shell = cx.global_mut::<Shell>();
        shell.preview = opened;
        shell.live_events = live;
    }
    cx.activate(true);
}

fn handle_status(event: StatusEvent, cx: &mut App) {
    match event {
        StatusEvent::TogglePreview => {
            // Hiding the app hides the window; its visibility observer
            // stops the camera.
            if preview_shown(cx) {
                cx.hide();
            } else {
                show_preview(cx);
            }
        }
        StatusEvent::Quit => cx.quit(),
    }
}

/// Stop the camera while the screen saver runs or the screen is locked.
fn handle_away(event: away::AwayEvent, cx: &mut App) {
    let shell = cx.global_mut::<Shell>();
    match event {
        away::AwayEvent::ScreenSaver(on) => shell.saver = on,
        away::AwayEvent::Locked(on) => shell.locked = on,
    }
    let is_away = shell.saver || shell.locked;
    if let Some((_, preview)) = shell.preview.clone() {
        preview.update(cx, |p, cx| p.set_away(is_away, cx));
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            cx.set_activation_policy(ActivationPolicy::Accessory);

            let mtm = MainThreadMarker::new().expect("GPUI runs the app on the main thread");
            let (tx, mut rx) = futures::channel::mpsc::unbounded();
            let status = status_item::install(mtm, tx);
            let (away_tx, mut away_rx) = futures::channel::mpsc::unbounded();
            let away = away::watch(away_tx);
            cx.set_global(Shell {
                preview: None,
                status,
                live_events: None,
                saver: false,
                locked: false,
                _away: away,
            });
            show_preview(cx);

            cx.spawn(async move |cx| {
                while let Some(event) = rx.next().await {
                    cx.update(|cx| handle_status(event, cx));
                }
            })
            .detach();
            cx.spawn(async move |cx| {
                while let Some(event) = away_rx.next().await {
                    cx.update(|cx| handle_away(event, cx));
                }
            })
            .detach();
        });
}

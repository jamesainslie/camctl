//! camctl: menu bar camera control with a floating live preview.

mod assets;
mod away;
mod events;
mod native;
mod prefs;
mod preview;
mod status_item;

use std::time::Duration;

use futures::StreamExt;
use gpui_kit::base::Root;
use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::{
    ActivationPolicy, AnyWindowHandle, App, AppContext, Bounds, Entity, Global, Subscription,
    WindowBounds, WindowKind, WindowOptions, px, size,
};
use objc2::MainThreadMarker;

use crate::preview::{Preview, PreviewEvent};
use crate::status_item::StatusEvent;

/// How long a camera switch waits for the old camera to stop.
const STOP_WITHIN: Duration = Duration::from_secs(5);

/// App-wide state: the preview window and the menu bar icon.
struct Shell {
    preview: Option<(AnyWindowHandle, Entity<Preview>)>,
    status: status_item::StatusItem,
    live_events: Option<Subscription>,
    /// Screen saver running, screen locked.
    saver: bool,
    locked: bool,
    /// A camera switch is waiting for the old camera to stop.
    switching: bool,
    /// Previews whose camera had not stopped when switching away from it.
    /// Dropping one would block the UI until the camera stopped, so each
    /// is kept until it does.
    stuck: Vec<Entity<Preview>>,
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
        let camera = prefs::camera();
        cx.new(|cx| Preview::new(away, camera, window, cx))
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
        if let Some((handle, preview)) = open_preview(cx) {
            adopt_preview(handle, preview, cx);
        } else {
            let shell = cx.global_mut::<Shell>();
            shell.preview = None;
            shell.live_events = None;
        }
    }
    cx.activate(true);
}

/// Make `preview` the one the menu bar icon follows.
fn adopt_preview(handle: AnyWindowHandle, preview: Entity<Preview>, cx: &mut App) {
    let live = cx.subscribe(&preview, |_, event: &PreviewEvent, cx| {
        let status = &cx.global::<Shell>().status;
        match event {
            PreviewEvent::Live(live) => status.set_live(*live),
            PreviewEvent::Asleep(asleep) => status.set_asleep(*asleep),
            PreviewEvent::Camera(id) => status.set_camera(Some(id)),
        }
    });
    let camera = preview.read(cx).camera_id().map(str::to_owned);
    let shell = cx.global_mut::<Shell>();
    // A new preview starts awake, with no frames yet.
    shell.status.set_asleep(false);
    shell.status.set_live(false);
    shell.status.set_camera(camera.as_deref());
    shell.preview = Some((handle, preview));
    shell.live_events = Some(live);
}

/// Preview and control another camera, and remember the choice. The old
/// camera stops first, off the UI thread; then a fresh preview for the new
/// one takes over the window, so nothing of the old camera's state (its
/// controls, position or sleep) carries over.
fn choose_camera(unique_id: &str, cx: &mut App) {
    // The menu was filled when it opened; the camera may have gone since.
    if !camctl_capture::devices()
        .iter()
        .any(|d| d.unique_id == unique_id)
    {
        events::record(&format!("camera [{unique_id}] went away before the switch"));
        return;
    }
    prefs::set_camera(unique_id);
    let shell = cx.global::<Shell>();
    // The switch under way opens whichever camera was chosen last.
    if shell.switching {
        return;
    }
    let Some((handle, preview)) = shell.preview.clone() else {
        show_preview(cx);
        return;
    };
    if preview.read(cx).camera_id() == Some(unique_id) {
        show_preview(cx);
        return;
    }
    events::record(&format!("switching to camera [{unique_id}]"));
    cx.global_mut::<Shell>().switching = true;
    let stopped = preview.update(cx, Preview::shut_down);
    cx.spawn(async move |cx| {
        let timeout = cx.background_executor().timer(STOP_WITHIN);
        let late = match futures::future::select(stopped, Box::pin(timeout)).await {
            futures::future::Either::Left(_) => None,
            futures::future::Either::Right(((), stopped)) => Some(stopped),
        };
        cx.update(|cx| replace_preview(handle, preview.clone(), late.is_none(), cx));
        // Release a camera that was slow to stop once it has stopped.
        if let Some(stopped) = late
            && stopped.await.is_ok()
        {
            events::record("the old camera stopped; releasing its preview");
            cx.update(|cx| {
                cx.global_mut::<Shell>()
                    .stuck
                    .retain(|stuck| stuck != &preview);
            });
        }
    })
    .detach();
}

/// Swap a fresh preview, for the chosen camera, into the preview window.
fn replace_preview(handle: AnyWindowHandle, old: Entity<Preview>, stopped: bool, cx: &mut App) {
    let shell = cx.global_mut::<Shell>();
    shell.switching = false;
    if !stopped {
        events::record("the old camera has not stopped; keeping its preview");
        shell.stuck.push(old);
    }
    let away = shell.saver || shell.locked;
    let camera = prefs::camera();
    let replaced = handle.update(cx, |_, window, cx| {
        let preview = cx.new(|cx| Preview::new(away, camera, window, cx));
        window.replace_root(cx, |window, cx| Root::new(preview.clone(), window, cx));
        preview
    });
    match replaced {
        Ok(preview) => adopt_preview(handle, preview, cx),
        // The window is gone: open a new one.
        Err(_) => cx.global_mut::<Shell>().preview = None,
    }
    show_preview(cx);
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
        StatusEvent::ChooseCamera(unique_id) => choose_camera(&unique_id, cx),
        StatusEvent::ToggleSleep => {
            // Mid-switch, the preview is the closing one.
            if cx.global::<Shell>().switching {
                return;
            }
            if let Some((_, preview)) = cx.global::<Shell>().preview.clone() {
                preview.update(cx, Preview::toggle_sleep);
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
                switching: false,
                stuck: Vec::new(),
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

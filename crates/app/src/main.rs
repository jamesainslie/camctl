//! camctl: menu bar camera control with a floating live preview.

mod assets;
mod away;
mod events;
mod instance;
mod native;
mod prefs;
mod preview;
mod status_item;
mod system;

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use camctl_capture::{Follow, Showing};
use futures::StreamExt;
use gpui_kit::base::Root;
use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::{
    ActivationPolicy, AnyWindowHandle, App, AppContext, Bounds, Entity, Global, Subscription,
    WindowBounds, WindowKind, WindowOptions, px, size,
};
use objc2::MainThreadMarker;

use crate::preview::{Preview, PreviewEvent, Sleep};
use crate::status_item::StatusEvent;
use crate::system::SystemEvent;

/// How long a camera switch waits for the old camera to stop.
const STOP_WITHIN: Duration = Duration::from_secs(5);
/// How long quitting waits for the camera to stop before leaving anyway.
const QUIT_WITHIN: Duration = Duration::from_secs(2);
/// How long the cameras must stay as they are before the preview follows
/// them: a camera re-enumerating disconnects and connects within a second
/// or so, and should be reopened once, not dropped and found again.
const DEVICES_SETTLE: Duration = Duration::from_secs(1);
/// How long the shown camera has to come back after it disconnects before
/// the preview falls back to another: a sleeping camera re-enumerating can
/// take a few seconds.
const SHOWN_CAMERA_SETTLE: Duration = Duration::from_secs(4);

/// App-wide state: the preview window and the menu bar icon.
#[expect(clippy::struct_excessive_bools, reason = "independent app state flags")]
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
    /// Bumped on each camera connect or disconnect; the preview follows
    /// the cameras once it stops changing.
    device_events: u64,
    /// When the cameras will have been still long enough to follow.
    settle_until: Instant,
    /// Cameras that connected since the preview last followed, by unique
    /// ID: the shown one among them came back with dead handles.
    connected: BTreeSet<String>,
    /// Each camera's sleep when its preview was last replaced, by unique
    /// ID, so a camera asleep when it went away (or was switched from)
    /// is still asleep when it is shown again.
    sleeps: HashMap<String, Sleep>,
    /// Quit is under way: nothing may open a camera.
    quitting: bool,
    _away: away::AwayWatch,
    _system: system::SystemWatch,
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
        cx.new(|cx| Preview::new(away, true, camera, Sleep::Awake, window, cx))
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
    let asleep = matches!(preview.read(cx).sleep_state(), Sleep::Asleep { .. });
    let shell = cx.global_mut::<Shell>();
    // A new preview has no frames yet.
    shell.status.set_asleep(asleep);
    shell.status.set_live(false);
    shell.status.set_camera(camera.as_deref());
    shell.preview = Some((handle, preview));
    shell.live_events = Some(live);
}

/// Preview and control another camera, and remember the choice.
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
    reopen_preview(handle, preview, true, cx);
}

/// Replace the preview with a fresh one, which opens the camera to show
/// now (see `camctl_capture::pick_camera`). The old camera stops first,
/// off the UI thread, so nothing of its state (its controls, position,
/// dead handles) carries over, except sleep: each camera's is remembered
/// and handed to the next preview that opens it. `show` brings the window
/// forward, for a reopen the user asked for.
fn reopen_preview(handle: AnyWindowHandle, preview: Entity<Preview>, show: bool, cx: &mut App) {
    if cx.global::<Shell>().quitting {
        return;
    }
    let old = preview.read(cx);
    let left = old.target_id().map(|id| (id.to_owned(), old.sleep_state()));
    let shell = cx.global_mut::<Shell>();
    shell.switching = true;
    if let Some((id, sleep)) = left {
        shell.sleeps.insert(id, sleep);
    }
    let stopped = preview.update(cx, Preview::shut_down);
    cx.spawn(async move |cx| {
        let timeout = cx.background_executor().timer(STOP_WITHIN);
        let late = match futures::future::select(stopped, Box::pin(timeout)).await {
            futures::future::Either::Left(_) => None,
            futures::future::Either::Right(((), stopped)) => Some(stopped),
        };
        cx.update(|cx| replace_preview(handle, preview.clone(), late.is_none(), show, cx));
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

/// The camera a fresh preview would open now.
fn camera_to_show() -> Option<String> {
    let devices = camctl_capture::devices();
    let ids: Vec<&str> = devices.iter().map(|d| d.unique_id.as_str()).collect();
    camctl_capture::pick_camera(&ids, prefs::camera().as_deref())
        .and_then(|i| ids.get(i).map(|id| (*id).to_owned()))
}

/// Swap a fresh preview into the preview window. It streams only if the
/// window can be seen; `show` brings the window forward first.
fn replace_preview(
    handle: AnyWindowHandle,
    old: Entity<Preview>,
    stopped: bool,
    show: bool,
    cx: &mut App,
) {
    let next = camera_to_show();
    let shell = cx.global_mut::<Shell>();
    shell.switching = false;
    if !stopped {
        events::record("the old camera has not stopped; keeping its preview");
        shell.stuck.push(old);
    }
    // Quitting stopped the old camera; nothing new may start.
    if shell.quitting {
        return;
    }
    let away = shell.saver || shell.locked;
    let sleep = next
        .and_then(|id| shell.sleeps.get(&id).copied())
        .unwrap_or(Sleep::Awake);
    let camera = prefs::camera();
    let replaced = handle.update(cx, |_, window, cx| {
        let visible = window.is_visible();
        let preview = cx.new(|cx| Preview::new(away, visible, camera, sleep, window, cx));
        window.replace_root(cx, |window, cx| Root::new(preview.clone(), window, cx));
        preview
    });
    match replaced {
        Ok(preview) => adopt_preview(handle, preview, cx),
        // The window is gone: a new one opens when next shown.
        Err(_) => cx.global_mut::<Shell>().preview = None,
    }
    if show {
        show_preview(cx);
    }
}

/// Follow the cameras once they have stopped changing for `settle`.
/// A later event can push the wait out but never cut it short, so the
/// shown camera keeps its longer grace when a wake or another camera's
/// event follows its disconnect.
fn devices_changed(settle: Duration, cx: &mut App) {
    let shell = cx.global_mut::<Shell>();
    shell.device_events += 1;
    shell.settle_until = shell.settle_until.max(Instant::now() + settle);
    let event = shell.device_events;
    wait_for_devices(event, cx);
}

/// Follow the cameras at `settle_until`, unless another event came since
/// `event`.
fn wait_for_devices(event: u64, cx: &mut App) {
    let wait = cx
        .global::<Shell>()
        .settle_until
        .saturating_duration_since(Instant::now());
    cx.spawn(async move |cx| {
        cx.background_executor().timer(wait).await;
        cx.update(|cx| {
            let shell = cx.global::<Shell>();
            if shell.device_events != event {
                return;
            }
            if Instant::now() < shell.settle_until {
                wait_for_devices(event, cx);
            } else {
                follow_devices(cx);
            }
        });
    })
    .detach();
}

/// Reopen the preview if the cameras connected now call for it: the one
/// it shows went away or came back, or the camera it should show (the
/// chosen one, or a USB camera over the built-in) arrived.
fn follow_devices(cx: &mut App) {
    if cx.global::<Shell>().quitting {
        return;
    }
    if cx.global::<Shell>().switching {
        // Look again once the switch is done.
        devices_changed(DEVICES_SETTLE, cx);
        return;
    }
    let connected = std::mem::take(&mut cx.global_mut::<Shell>().connected);
    // No window yet: the preview picks a camera when it opens.
    let Some((handle, preview)) = cx.global::<Shell>().preview.clone() else {
        return;
    };
    let devices = camctl_capture::devices();
    let ids: Vec<&str> = devices.iter().map(|d| d.unique_id.as_str()).collect();
    let shown = preview.read(cx);
    let target = shown.target_id().map(str::to_owned);
    let showing = Showing {
        target: target.as_deref(),
        healthy: shown.camera_id().is_some(),
        reconnected: target.as_ref().is_some_and(|t| connected.contains(t)),
    };
    if camctl_capture::follow(&ids, prefs::camera().as_deref(), showing) == Follow::Reopen {
        events::record(&format!(
            "cameras changed ({} connected); reopening the preview",
            ids.len()
        ));
        // Not brought forward: the camera follows the window, so a hidden
        // preview stays hidden and the camera stays off.
        reopen_preview(handle, preview, false, cx);
    }
}

/// The camera the preview is for, if any.
fn shown_target(cx: &App) -> Option<String> {
    let (_, preview) = cx.global::<Shell>().preview.as_ref()?;
    preview.read(cx).target_id().map(str::to_owned)
}

fn handle_system(event: SystemEvent, cx: &mut App) {
    match event {
        SystemEvent::CameraConnected(id) => {
            events::record(&format!(
                "camera connected [{}]",
                id.as_deref().unwrap_or("?")
            ));
            if let Some(id) = id {
                cx.global_mut::<Shell>().connected.insert(id);
            }
            devices_changed(DEVICES_SETTLE, cx);
        }
        SystemEvent::CameraDisconnected(id) => {
            events::record(&format!(
                "camera disconnected [{}]",
                id.as_deref().unwrap_or("?")
            ));
            // The shown camera gets longer to come back before the preview
            // falls back to another.
            let settle = if id.is_some() && id == shown_target(cx) {
                SHOWN_CAMERA_SETTLE
            } else {
                DEVICES_SETTLE
            };
            devices_changed(settle, cx);
        }
        SystemEvent::Woke => {
            events::record("the Mac woke");
            let shell = cx.global::<Shell>();
            if shell.quitting {
                return;
            }
            // USB handles may be dead before any transfer says so.
            if !shell.switching
                && let Some((_, preview)) = shell.preview.clone()
            {
                preview.update(cx, |p, cx| p.reconnect_controls("the Mac woke", cx));
            }
            devices_changed(DEVICES_SETTLE, cx);
        }
        SystemEvent::ShowRequested => {
            events::record("another launch asked to show the preview");
            show_preview(cx);
        }
    }
}

/// Stop the camera, then quit. A camera that does not stop in time would
/// hang the quit, so camctl leaves without it.
fn quit(cx: &mut App) {
    events::record("quitting");
    // Nothing may open a camera from here on.
    cx.global_mut::<Shell>().quitting = true;
    let Some((_, preview)) = cx.global::<Shell>().preview.clone() else {
        cx.quit();
        return;
    };
    let stopped = preview.update(cx, Preview::shut_down);
    cx.spawn(async move |cx| {
        let timeout = cx.background_executor().timer(QUIT_WITHIN);
        let in_time = matches!(
            futures::future::select(stopped, Box::pin(timeout)).await,
            futures::future::Either::Left((Ok(()), _))
        );
        cx.update(|cx| {
            // Dropping a camera that has not stopped blocks until it does.
            if !in_time || !cx.global::<Shell>().stuck.is_empty() {
                events::record("a camera has not stopped; quitting without it");
                std::process::exit(0);
            }
            cx.quit();
        });
    })
    .detach();
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
        StatusEvent::Reconnect => {
            if cx.global::<Shell>().switching {
                return;
            }
            events::record("reconnect asked for from the menu");
            match cx.global::<Shell>().preview.clone() {
                Some((handle, preview)) => reopen_preview(handle, preview, true, cx),
                None => show_preview(cx),
            }
        }
        StatusEvent::ToggleSleep => {
            // Mid-switch, the preview is the closing one.
            if cx.global::<Shell>().switching {
                return;
            }
            if let Some((_, preview)) = cx.global::<Shell>().preview.clone() {
                preview.update(cx, Preview::toggle_sleep);
            }
        }
        StatusEvent::Quit => quit(cx),
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

/// Whether this launch is the one camctl.
enum Claim {
    Sole(instance::InstanceLock),
    /// Another camctl runs; it has been asked to show itself.
    Handed,
    /// The lock could not be taken at all: better two camctls than none.
    Unlocked,
}

/// Take the one-camctl lock, or hand over to the camctl already running.
fn claim_instance() -> Claim {
    let Some(path) = instance::default_path() else {
        return Claim::Unlocked;
    };
    match instance::acquire(&path) {
        Ok(Some(lock)) => Claim::Sole(lock),
        Ok(None) => {
            events::record("camctl is already running; asking it to show its preview");
            system::ask_running_instance_to_show();
            Claim::Handed
        }
        Err(e) => {
            events::record(&format!(
                "could not take the instance lock ({e}); running anyway"
            ));
            Claim::Unlocked
        }
    }
}

fn main() {
    // Held for the life of the process; the kernel releases it on exit.
    static INSTANCE: std::sync::OnceLock<instance::InstanceLock> = std::sync::OnceLock::new();
    match claim_instance() {
        Claim::Sole(lock) => {
            let _ = INSTANCE.set(lock);
        }
        Claim::Handed => return,
        Claim::Unlocked => {}
    }
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
            let (system_tx, mut system_rx) = futures::channel::mpsc::unbounded();
            let system = system::watch(system_tx);
            cx.set_global(Shell {
                preview: None,
                status,
                live_events: None,
                saver: false,
                locked: false,
                switching: false,
                stuck: Vec::new(),
                device_events: 0,
                settle_until: Instant::now(),
                connected: BTreeSet::new(),
                sleeps: HashMap::new(),
                quitting: false,
                _away: away,
                _system: system,
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
            cx.spawn(async move |cx| {
                while let Some(event) = system_rx.next().await {
                    cx.update(|cx| handle_system(event, cx));
                }
            })
            .detach();
        });
}

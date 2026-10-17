//! The floating preview window: live video, and pan, tilt and zoom from
//! the keyboard, the mouse and the scroll wheel.
//!
//! Every input becomes either a held "stick" (keys, the on-screen pad, a
//! drag on the video) that `Motion` turns into smoothly advancing targets,
//! or a one-shot target (a click to aim, a scroll step, Home).
//!
//! Releases are caught at window level, whatever the button and wherever
//! the pointer ends up, so a held move can never be left running.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use camctl_capture::policy::Want;
use camctl_capture::{Authorization, Frame, Session};
use camctl_core::motion::{Limits, Motion, Rig, Speeds, Stick, Targets, cover_offset, drag_stick};
use camctl_core::{ControlQueue, QueueEvent, quirks};
use camctl_uvc::{Control, Value};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::assets::IconName;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme, Icon, Sizable, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext, Bounds, Context, CursorStyle, DispatchPhase, Div, Entity, EventEmitter,
    FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, Modifiers,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit,
    ParentElement, Pixels, Point, Render, Result, ScrollDelta, ScrollWheelEvent, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Subscription, Task, Window, canvas, div, hsla,
    px, surface,
};

/// How often held input advances the targets.
const TICK: Duration = Duration::from_millis(33);
const SPEEDS: Speeds = Speeds {
    pan: 45.0,
    tilt: 35.0,
    zoom: 60.0,
    ramp_seconds: 0.25,
};
/// Shift slows arrow-key moves to this fraction, for fine framing.
const SLOW: f64 = 0.3;
/// Drag distance, in pixels, for full speed.
const DRAG_RADIUS: f64 = 140.0;
/// Pointer travel, in pixels, that turns a click into a drag.
const CLICK_SLOP: f64 = 5.0;
const ZOOM_PER_LINE: f64 = 4.0;
const ZOOM_PER_PIXEL: f64 = 0.15;
/// While idle, read the camera's position this often: it can move on its
/// own (OBSBOT's on-device tracking) and aiming is relative to it.
const SYNC_EVERY: Duration = Duration::from_secs(2);
/// Don't read the position back until the motor has had time to finish a
/// move, or the read would catch it halfway.
const SETTLE: Duration = Duration::from_millis(1500);
/// How long the click marker stays.
const MARKER_FOR: Duration = Duration::from_millis(600);
/// A stream that delivers no frame for this long, at its start or later,
/// is restarted.
const STALL_AFTER: Duration = Duration::from_secs(4);
/// Restarts before giving up: a quick stop and start, then rebuilds.
const MAX_RESTARTS: u32 = 3;
/// How long a rebuild waits for the old session to finish stopping.
const IDLE_WITHIN: Duration = Duration::from_secs(5);
/// Cell size of the on-video pad.
const PAD_CELL: f32 = 30.0;

/// Emitted when frames start or stop flowing, for the menu bar icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewEvent {
    Live(bool),
}

#[derive(Debug, Clone, PartialEq)]
enum Status {
    AskingPermission,
    Denied,
    NoCamera,
    Starting,
    /// Deliberately not streaming, and why.
    Paused(&'static str),
    Live,
    Failed(SharedString),
}

/// A press on the video that may become a drag.
#[derive(Debug, Clone, Copy)]
struct Press {
    start: Point<Pixels>,
    dragging: bool,
    stick: Stick,
}

#[expect(clippy::struct_excessive_bools, reason = "independent UI state flags")]
pub struct Preview {
    status: Status,
    device_name: SharedString,
    frame: Option<Frame>,
    session: Option<Session>,
    queue: Option<ControlQueue>,
    motion: Option<Motion>,
    can_aim: bool,
    can_zoom: bool,
    zoom: Option<Entity<SliderState>>,
    /// Movement keys held down, and whether shift is down.
    keys: BTreeSet<&'static str>,
    slow: bool,
    /// An on-screen pad button held down.
    button: Option<Stick>,
    press: Option<Press>,
    marker: Option<Point<Pixels>>,
    marker_task: Option<Task<()>>,
    video_bounds: Rc<Cell<Bounds<Pixels>>>,
    ticking: bool,
    last_tick: Instant,
    /// Writes sent so far, when the last one went, and the write count a
    /// pending read was asked at: a read is only trusted if nothing was
    /// written after it was requested.
    writes: u64,
    last_write: Instant,
    read_epoch: Option<u64>,
    last_zoom_sent: Option<i64>,
    focus: FocusHandle,
    wake: UnboundedSender<()>,
    events: UnboundedSender<QueueEvent>,
    running: bool,
    /// The capture device, kept so a stuck session can be rebuilt.
    device: Option<camctl_capture::Device>,
    /// Frames delivered at the last liveness check, the restarts tried for
    /// the current stall, the liveness timer, and whether a rebuild is
    /// waiting for the old session to stop.
    watch_delivered: u64,
    restarts: u32,
    watchdog: Option<Task<()>>,
    rebuilding: bool,
    /// The window can be seen, and the user is not away (screen saver or
    /// lock screen). The camera streams only when both hold.
    visible: bool,
    away: bool,
    tasks: Vec<Task<()>>,
    subscriptions: Vec<Subscription>,
}

impl EventEmitter<PreviewEvent> for Preview {}

fn trace(step: &str) {
    crate::events::record(step);
}

fn movement_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "left" => "left",
        "right" => "right",
        "up" => "up",
        "down" => "down",
        "=" | "+" => "zoom-in",
        "-" => "zoom-out",
        _ => return None,
    })
}

/// Command, Control or Option: the key belongs to a shortcut, not to us.
fn is_shortcut(m: Modifiers) -> bool {
    m.platform || m.control || m.alt
}

/// Whole degrees, without a "-0".
fn degrees(v: f64) -> String {
    let rounded = v.round();
    format!("{}°", if rounded == 0.0 { 0.0 } else { rounded })
}

fn px_f64(p: Pixels) -> f64 {
    f64::from(f32::from(p))
}

impl Preview {
    /// `away` is whether the screen saver or lock screen is up right now.
    pub fn new(away: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (wake, mut woken) = unbounded::<()>();
        // Frames: the capture queue pokes `wake` once per pending frame;
        // take it here, on the UI thread, and redraw.
        let frames = cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                if this.update(cx, Preview::take_frame).is_err() {
                    break;
                }
            }
        });
        // Control results from the worker thread.
        let (events, mut results) = unbounded::<QueueEvent>();
        let results = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = results.next().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        this.on_queue_event(event, window, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Never leave the camera moving when the window loses focus and its
        // key-up never arrives.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                // A window the user is using is visible, whatever the last
                // occlusion report said.
                if !this.visible {
                    trace("window activated: treating it as visible");
                    this.visible = true;
                    this.refresh_streaming(cx);
                }
            } else {
                this.release_all(window, cx);
                // Back to what the window server says, so a window covered
                // later is still noticed.
                this.visible = window.is_visible();
                this.refresh_streaming(cx);
            }
        });
        // Stream only while the window can be seen: hidden, minimized,
        // covered or on a sleeping display all stop the camera.
        let weak = cx.weak_entity();
        let visibility = window.observe_window_visibility(move |visibility, _, cx| {
            let _ = weak.update(cx, |this, cx| {
                let visible = visibility.is_visible();
                if visible != this.visible {
                    trace(if visible {
                        "window became visible"
                    } else {
                        "window hidden or covered"
                    });
                }
                this.visible = visible;
                this.refresh_streaming(cx);
            });
        });
        let mut preview = Preview {
            status: Status::Starting,
            device_name: "camctl".into(),
            frame: None,
            session: None,
            queue: None,
            motion: None,
            can_aim: false,
            can_zoom: false,
            zoom: None,
            keys: BTreeSet::new(),
            slow: false,
            button: None,
            press: None,
            marker: None,
            marker_task: None,
            video_bounds: Rc::new(Cell::new(Bounds::default())),
            ticking: false,
            last_tick: Instant::now(),
            writes: 0,
            last_write: Instant::now(),
            read_epoch: None,
            last_zoom_sent: None,
            focus,
            wake,
            events,
            running: false,
            device: None,
            watch_delivered: 0,
            restarts: 0,
            watchdog: None,
            rebuilding: false,
            visible: true,
            away,
            tasks: vec![frames, results],
            subscriptions: vec![activation, visibility],
        };
        trace("authorization");
        match camctl_capture::authorization() {
            Authorization::Authorized => preview.open_camera(cx),
            Authorization::NotDetermined => preview.ask_permission(cx),
            Authorization::Denied | Authorization::Restricted => preview.status = Status::Denied,
        }
        preview.start_sync(cx);
        preview
    }

    fn ask_permission(&mut self, cx: &mut Context<Self>) {
        trace("ask_permission");
        self.status = Status::AskingPermission;
        let (tx, mut rx) = unbounded::<bool>();
        camctl_capture::request_access(move |granted| {
            let _ = tx.unbounded_send(granted);
        });
        let task = cx.spawn(async move |this, cx| {
            if let Some(granted) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    if granted {
                        this.open_camera(cx);
                    } else {
                        this.status = Status::Denied;
                    }
                    cx.notify();
                });
            }
        });
        self.tasks.push(task);
    }

    fn open_camera(&mut self, cx: &mut Context<Self>) {
        trace("open_camera");
        let devices = camctl_capture::devices();
        let Some(device) = devices
            .iter()
            .find(|d| d.kind() == camctl_capture::DeviceKind::Usb)
            .or_else(|| devices.first())
        else {
            self.status = Status::NoCamera;
            return;
        };
        self.device_name = device.name.clone().into();
        trace(&format!("camera {} [{}]", device.name, device.unique_id));
        self.device = Some(device.clone());
        if !self.build_session(cx) {
            return;
        }
        if let Some(identity) = camctl_capture::usb_identity(&device.unique_id) {
            trace("open_controls");
            self.open_controls(identity, cx);
        }
        trace("camera open");
    }

    /// A new capture session for the device, configured and started if the
    /// window is up and the user is here. Returns whether it was built.
    fn build_session(&mut self, cx: &mut Context<Self>) -> bool {
        let built = self.try_build_session(cx);
        cx.notify();
        built
    }

    fn try_build_session(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(old) = self.device.clone() else {
            return false;
        };
        // Look the camera up again: if it reconnected (during sleep, say)
        // the old handle is dead.
        let Some(device) = camctl_capture::devices()
            .into_iter()
            .find(|d| d.unique_id == old.unique_id)
        else {
            trace("camera no longer connected");
            self.session = None;
            self.running = false;
            self.status = Status::NoCamera;
            return false;
        };
        self.device = Some(device.clone());
        // Drop any old session first: its drop stops it and waits for its
        // queue, so the camera is free for the new one. Callers make sure
        // that queue is idle, so this does not block.
        self.session = None;
        self.running = false;
        let wake = self.wake.clone();
        match Session::new(&device, move || {
            let _ = wake.unbounded_send(());
        }) {
            Ok(session) => {
                match session.configure_for(Want {
                    width: 960,
                    height: 540,
                    max_width: 1920,
                }) {
                    Ok(Some(c)) => trace(&format!(
                        "streaming {:?} {}x{}",
                        c.encoding, c.width, c.height
                    )),
                    Ok(None) => trace("keeping the current format"),
                    Err(e) => trace(&format!("format not set: {e}")),
                }
                self.session = Some(session);
                self.status = Status::Starting;
                // Start only if the window is up and the user is here.
                self.refresh_streaming(cx);
                if !self.running {
                    self.status = Status::Paused(self.pause_reason());
                }
                true
            }
            Err(e) => {
                trace(&format!("capture session failed: {e}"));
                self.status = Status::Failed(e.to_string().into());
                false
            }
        }
    }

    /// While streaming, check that frames keep arriving; on a stall,
    /// restart, then rebuild, then give up and say so. Returns whether to
    /// keep watching.
    fn check_liveness(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.running || self.rebuilding {
            return false;
        }
        let Some(session) = &self.session else {
            return false;
        };
        let delivered = session.frames_delivered();
        if delivered > self.watch_delivered {
            self.watch_delivered = delivered;
            self.restarts = 0;
            return true;
        }
        if self.restarts >= MAX_RESTARTS {
            trace("no frames after restarting; giving up");
            // Stop, so the camera light goes off; the next resume (showing
            // the window again, say) tries afresh.
            session.stop();
            self.running = false;
            self.restarts = 0;
            self.frame = None;
            self.status = Status::Failed(
                "The camera sends no video. Unplug it and plug it back in, or quit the app using it."
                    .into(),
            );
            cx.notify();
            return false;
        }
        self.restarts += 1;
        trace(&format!(
            "no frames for {}s; restart {} of {MAX_RESTARTS}",
            STALL_AFTER.as_secs(),
            self.restarts
        ));
        if self.restarts == 1 {
            session.stop();
            session.start_fresh();
            self.watch_delivered = session.frames_delivered();
            true
        } else {
            self.rebuild(cx);
            false
        }
    }

    /// Replace a stuck session without blocking the UI: stop it, wait (off
    /// the UI thread, with a timeout) for its queue to finish, then build a
    /// new one.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            self.build_session(cx);
            return;
        };
        self.rebuilding = true;
        session.stop();
        let (tx, rx) = futures::channel::oneshot::channel::<()>();
        session.when_idle(move || {
            let _ = tx.send(());
        });
        let task = cx.spawn(async move |this, cx| {
            let timeout = cx.background_executor().timer(IDLE_WITHIN);
            let idle = matches!(
                futures::future::select(rx, Box::pin(timeout)).await,
                futures::future::Either::Left((Ok(()), _))
            );
            let _ = this.update(cx, |this, cx| {
                this.rebuilding = false;
                if idle {
                    this.build_session(cx);
                } else {
                    trace("camera did not stop; giving up");
                    this.running = false;
                    this.status = Status::Failed(
                        "The camera stopped responding. Unplug it and plug it back in.".into(),
                    );
                    cx.notify();
                }
            });
        });
        self.tasks.push(task);
    }

    /// Watch the stream for stalls while it runs. Replacing the task
    /// cancels the previous watch (async-task allows a running task to
    /// drop its own handle; it finishes its current step).
    fn arm_watchdog(&mut self, cx: &mut Context<Self>) {
        self.watch_delivered = self.session.as_ref().map_or(0, Session::frames_delivered);
        self.watchdog = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(STALL_AFTER).await;
                let keep = this.update(cx, Preview::check_liveness).unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    /// Pair the preview device with its USB device and take over control.
    /// USB enumeration and the first reads run off the UI thread; a camera
    /// that is busy starting its stream can take a few hundred milliseconds
    /// to answer.
    fn open_controls(&mut self, identity: (u32, u16, u16), cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            let opened = cx
                .background_spawn(async move { open_usb_camera(identity) })
                .await;
            let Some((camera, initial)) = opened else {
                trace("no USB control for this camera");
                return;
            };
            let _ = this.update(cx, |this, cx| this.controls_ready(camera, initial, cx));
        });
        self.tasks.push(task);
    }

    fn controls_ready(
        &mut self,
        camera: UsbCamera,
        Initial {
            rig,
            can_aim,
            can_zoom,
            zoom_step,
            vendor_id,
            product_id,
        }: Initial,
        cx: &mut Context<Self>,
    ) {
        self.can_aim = can_aim;
        self.can_zoom = can_zoom;
        if can_zoom {
            let z = rig.zoom_limits;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "zoom units are small integers"
            )]
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(z.min() as f32)
                    .max(z.max() as f32)
                    .step(zoom_step as f32)
                    .default_value(rig.zoom as f32)
            });
            let sub = cx.subscribe(&slider, |this, _, event: &SliderEvent, cx| {
                let (SliderEvent::Change(v) | SliderEvent::Release(v)) = event;
                this.zoom_to(f64::from(v.start()), cx);
            });
            self.subscriptions.push(sub);
            self.zoom = Some(slider);
        }
        self.motion = Some(Motion::new(rig, SPEEDS, quirks::fov(vendor_id, product_id)));
        let events = self.events.clone();
        self.queue = Some(ControlQueue::spawn(camera, move |event| {
            if let QueueEvent::Wrote(w) = &event
                && let Err(e) = &w.result
            {
                trace(&format!("{} = {:?} failed: {e}", w.control.key(), w.value));
            }
            let _ = events.unbounded_send(event);
        }));
        trace("controls ready");
        cx.notify();
    }

    /// Adopt positions the camera reports, unless the user moved it since
    /// the read was asked for, or is moving it now.
    fn on_queue_event(&mut self, event: QueueEvent, window: &mut Window, cx: &mut Context<Self>) {
        let QueueEvent::Read(read) = event else {
            return;
        };
        if self.ticking || self.read_epoch != Some(self.writes) {
            return;
        }
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        if motion.is_active() {
            return;
        }
        let rig = *motion.rig();
        match (read.control, read.result) {
            (Control::PanTiltAbsolute, Ok(v)) => {
                if let Some((pan, tilt)) = v.pan_tilt_degrees() {
                    motion.sync(pan, tilt, rig.zoom);
                }
            }
            (Control::ZoomAbsolute, Ok(Value::Int(z))) => {
                #[expect(clippy::cast_precision_loss, reason = "zoom units are small integers")]
                motion.sync(rig.pan, rig.tilt, z as f64);
                self.last_zoom_sent = Some(z);
                self.sync_slider(window, cx);
            }
            _ => return,
        }
        cx.notify();
    }

    /// Read the camera's position now and then while the preview streams
    /// and the user has left it alone for a moment.
    fn start_sync(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SYNC_EVERY).await;
                let alive = this.update(cx, |this, _| {
                    if this.running
                        && !this.ticking
                        && this.last_write.elapsed() >= SETTLE
                        && let Some(queue) = &this.queue
                    {
                        this.read_epoch = Some(this.writes);
                        if this.can_aim {
                            queue.read(Control::PanTiltAbsolute);
                        }
                        if this.can_zoom {
                            queue.read(Control::ZoomAbsolute);
                        }
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        self.tasks.push(task);
    }

    fn take_frame(&mut self, cx: &mut Context<Self>) {
        // Always empty the slot, even when paused: a frame left in it would
        // stop every later frame from waking us.
        let frame = self.session.as_ref().and_then(Session::take_frame);
        if !self.running {
            return;
        }
        let Some(frame) = frame else {
            return;
        };
        self.frame = Some(frame);
        if self.status != Status::Live {
            self.status = Status::Live;
            trace("first frame");
            cx.emit(PreviewEvent::Live(true));
        }
        cx.notify();
    }

    /// The screen saver or lock screen came or went.
    pub fn set_away(&mut self, away: bool, cx: &mut Context<Self>) {
        if away != self.away {
            trace(if away {
                "user away (screen saver or lock)"
            } else {
                "user back"
            });
        }
        self.away = away;
        self.refresh_streaming(cx);
    }

    /// Stream exactly when the window can be seen and the user is here.
    fn refresh_streaming(&mut self, cx: &mut Context<Self>) {
        if self.visible && !self.away {
            self.resume(cx);
        } else {
            self.pause(self.pause_reason(), cx);
        }
    }

    fn pause_reason(&self) -> &'static str {
        if self.away {
            "Paused while the screen saver or lock screen is up."
        } else {
            "Paused while the window is hidden or covered."
        }
    }

    /// Stop streaming: the camera light goes off and nothing runs.
    fn pause(&mut self, why: &'static str, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        if !self.running {
            return;
        }
        session.stop();
        self.running = false;
        self.watchdog = None;
        // A stall interrupted by a pause starts its restarts afresh.
        self.restarts = 0;
        // Don't show a stale picture when the window comes back.
        self.frame = None;
        let was_live = self.status == Status::Live;
        if matches!(self.status, Status::Live | Status::Starting) {
            self.status = Status::Paused(why);
        }
        if was_live {
            cx.emit(PreviewEvent::Live(false));
        }
        trace(&format!("paused: {why}"));
        cx.notify();
    }

    fn resume(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        if self.running {
            return;
        }
        session.start_fresh();
        self.running = true;
        if matches!(self.status, Status::Paused(_) | Status::Failed(_)) {
            self.status = Status::Starting;
        }
        self.arm_watchdog(cx);
        trace("resumed");
        cx.notify();
    }

    // --- Motion -------------------------------------------------------------

    fn send(&mut self, targets: Targets) {
        let Some(queue) = &self.queue else {
            return;
        };
        let mut wrote = false;
        if let Some((pan, tilt)) = targets.pan_tilt
            && let Ok(v) = Value::pan_tilt_from_degrees(pan, tilt)
        {
            queue.set(Control::PanTiltAbsolute, v);
            wrote = true;
        }
        if let Some(zoom) = targets.zoom {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "zoom is clamped to the camera's small integer range"
            )]
            let z = zoom.round() as i64;
            // A slow zoom rounds to the same unit for several ticks.
            if self.last_zoom_sent != Some(z) {
                queue.set(Control::ZoomAbsolute, Value::Int(z));
                self.last_zoom_sent = Some(z);
                wrote = true;
            }
        }
        if wrote {
            self.writes += 1;
            self.last_write = Instant::now();
        }
    }

    /// Combine the held inputs into one stick: a drag wins, then a pad
    /// button, then the keyboard.
    fn held_stick(&self) -> Stick {
        if let Some(press) = self.press.filter(|p| p.dragging) {
            return press.stick;
        }
        if let Some(button) = self.button {
            return button;
        }
        let mut s = Stick::default();
        for key in &self.keys {
            match *key {
                "left" => s.right -= 1.0,
                "right" => s.right += 1.0,
                "up" => s.up += 1.0,
                "down" => s.up -= 1.0,
                "zoom-in" => s.zoom_in += 1.0,
                "zoom-out" => s.zoom_in -= 1.0,
                _ => {}
            }
        }
        // Shift slows aiming only: `+` itself needs shift on many layouts.
        if self.slow {
            s.right *= SLOW;
            s.up *= SLOW;
        }
        s
    }

    fn update_stick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stick = self.held_stick();
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        motion.set_stick(stick);
        if motion.is_active() && !self.ticking {
            self.ticking = true;
            self.last_tick = Instant::now();
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(TICK).await;
                    let more = this.update_in(cx, Preview::tick).unwrap_or(false);
                    if !more {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    /// One step of held motion. Returns whether to keep ticking.
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let now = Instant::now();
        let dt = (now - self.last_tick).as_secs_f64().min(0.1);
        self.last_tick = now;
        let Some(motion) = self.motion.as_mut() else {
            self.ticking = false;
            return false;
        };
        let targets = motion.tick(dt);
        let more = motion.is_active();
        if let Some(t) = targets {
            self.send(t);
            if t.zoom.is_some() {
                self.sync_slider(window, cx);
            }
            cx.notify();
        }
        if !more {
            self.ticking = false;
        }
        more
    }

    fn release_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.keys.clear();
        self.button = None;
        self.press = None;
        self.update_stick(window, cx);
    }

    fn sync_slider(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(slider), Some(motion)) = (&self.zoom, &self.motion) {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "zoom units are small integers"
            )]
            let z = motion.rig().zoom as f32;
            slider.update(cx, |s, cx| s.set_value(z, window, cx));
        }
    }

    fn zoom_to(&mut self, zoom: f64, cx: &mut Context<Self>) {
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        let delta = zoom - motion.rig().zoom;
        if let Some(z) = motion.zoom_by(delta) {
            self.send(Targets {
                pan_tilt: None,
                zoom: Some(z),
            });
            cx.notify();
        }
    }

    fn zoom_by(&mut self, delta: f64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        if let Some(z) = motion.zoom_by(delta) {
            self.send(Targets {
                pan_tilt: None,
                zoom: Some(z),
            });
            self.sync_slider(window, cx);
            cx.notify();
        }
    }

    fn home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        motion.home();
        let r = *motion.rig();
        self.keys.clear();
        self.button = None;
        self.send(Targets {
            pan_tilt: self.can_aim.then_some((r.pan, r.tilt)),
            zoom: self.can_zoom.then_some(r.zoom),
        });
        self.sync_slider(window, cx);
        cx.notify();
    }

    /// Turn the camera to centre the point clicked at `position`. The
    /// camera's continuous autofocus then refocuses on what is now centred.
    fn aim_at(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let bounds = self.video_bounds.get();
        let (fw, fh) = self.frame.as_ref().map_or((16.0, 9.0), |f| {
            let (w, h) = f.size();
            #[expect(clippy::cast_precision_loss, reason = "frame sizes fit in f64")]
            (w as f64, h as f64)
        });
        let view = (px_f64(bounds.size.width), px_f64(bounds.size.height));
        let point = (
            px_f64(position.x - bounds.origin.x),
            px_f64(position.y - bounds.origin.y),
        );
        let Some((x, y)) = cover_offset(view, (fw, fh), point) else {
            return;
        };
        let Some(motion) = self.motion.as_mut() else {
            return;
        };
        if let Some(aim) = motion.aim_at(x, y, fw / fh) {
            self.send(Targets {
                pan_tilt: Some(aim),
                zoom: None,
            });
            self.marker = Some(position);
            // Replacing the task cancels the previous marker's timer.
            self.marker_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(MARKER_FOR).await;
                let _ = this.update(cx, |this, cx| {
                    this.marker = None;
                    cx.notify();
                });
            }));
            cx.notify();
        }
    }

    // --- Input ----------------------------------------------------------------

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        if is_shortcut(*modifiers) {
            return;
        }
        let raw = event.keystroke.key.as_str();
        if raw == "h" || raw == "0" {
            self.home(window, cx);
            cx.stop_propagation();
            return;
        }
        let Some(key) = movement_key(raw) else {
            return;
        };
        cx.stop_propagation();
        if event.is_held {
            return;
        }
        self.slow = modifiers.shift;
        self.keys.insert(key);
        self.update_stick(window, cx);
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = movement_key(event.keystroke.key.as_str())
            && self.keys.remove(key)
        {
            self.update_stick(window, cx);
        }
    }

    /// macOS sends no key-up while Command is held, so drop held keys when
    /// a shortcut modifier goes down; and follow Shift mid-hold.
    fn on_modifiers(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_shortcut(event.modifiers) {
            self.keys.clear();
        }
        self.slow = event.modifiers.shift;
        self.update_stick(window, cx);
    }

    fn on_video_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.press = Some(Press {
            start: event.position,
            dragging: false,
            stick: Stick::default(),
        });
    }

    /// Steers a drag, wherever in the window the pointer is.
    fn on_drag_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut press) = self.press else {
            return;
        };
        if event.pressed_button.is_none() {
            // Released somewhere we did not hear about.
            self.press = None;
            self.update_stick(window, cx);
            return;
        }
        let dx = px_f64(event.position.x - press.start.x);
        let dy = px_f64(event.position.y - press.start.y);
        if !press.dragging && dx.hypot(dy) > CLICK_SLOP && self.can_aim {
            press.dragging = true;
        }
        if press.dragging {
            press.stick = drag_stick(dx, dy, DRAG_RADIUS);
            self.press = Some(press);
            self.update_stick(window, cx);
        } else {
            self.press = Some(press);
        }
    }

    /// Any button released anywhere ends a drag or a pad press; a short
    /// press on the video is a click to aim.
    fn on_release(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let had_button = self.button.take().is_some();
        let press = self.press.take();
        match press {
            Some(p) if p.dragging => self.update_stick(window, cx),
            Some(_) if self.can_aim && self.video_bounds.get().contains(&event.position) => {
                self.aim_at(event.position, cx);
            }
            _ => {}
        }
        if had_button {
            self.update_stick(window, cx);
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = match event.delta {
            ScrollDelta::Lines(p) => f64::from(p.y) * ZOOM_PER_LINE,
            ScrollDelta::Pixels(p) => px_f64(p.y) * ZOOM_PER_PIXEL,
        };
        if delta != 0.0 && self.can_zoom {
            self.zoom_by(delta, window, cx);
            cx.stop_propagation();
        }
    }

    fn press_button(&mut self, stick: Stick, window: &mut Window, cx: &mut Context<Self>) {
        self.button = Some(stick);
        self.update_stick(window, cx);
    }

    // --- Rendering ------------------------------------------------------------

    fn message(&self) -> Option<(IconName, &'static str, SharedString)> {
        Some(match &self.status {
            Status::Live => return None,
            Status::Starting => (IconName::Video, "Starting the camera", "".into()),
            Status::Paused(why) => (IconName::Video, "Paused", (*why).into()),
            Status::AskingPermission => (
                IconName::Camera,
                "Camera access",
                "Allow camctl to use the camera to see the preview.".into(),
            ),
            Status::Denied => (
                IconName::Camera,
                "Camera access is off",
                "Turn on camctl in System Settings, Privacy & Security, Camera.".into(),
            ),
            Status::NoCamera => (IconName::Video, "No camera connected", "".into()),
            Status::Failed(why) => (IconName::Video, "The camera did not start", why.clone()),
        })
    }

    /// A round pad button that moves while held. The release is caught at
    /// window level by `on_release`.
    fn hold_button(
        id: &'static str,
        icon: IconName,
        stick: Stick,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        pad_button(id, icon).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                this.press_button(stick, window, cx);
                cx.stop_propagation();
            }),
        )
    }

    fn pad(&self, cx: &mut Context<Self>) -> Div {
        let s = |right: f64, up: f64, zoom_in: f64| Stick { right, up, zoom_in };
        let gap = || div().size(px(PAD_CELL));
        h_flex()
            .gap_3()
            .items_center()
            .when(self.can_aim, |this| {
                this.child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap_0p5()
                                .child(gap())
                                .child(Self::hold_button(
                                    "up",
                                    IconName::ChevronUp,
                                    s(0.0, 1.0, 0.0),
                                    cx,
                                ))
                                .child(gap()),
                        )
                        .child(
                            h_flex()
                                .gap_0p5()
                                .child(Self::hold_button(
                                    "left",
                                    IconName::ChevronLeft,
                                    s(-1.0, 0.0, 0.0),
                                    cx,
                                ))
                                .child(
                                    pad_button("home", IconName::LocateFixed)
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.home(window, cx);
                                        })),
                                )
                                .child(Self::hold_button(
                                    "right",
                                    IconName::ChevronRight,
                                    s(1.0, 0.0, 0.0),
                                    cx,
                                )),
                        )
                        .child(
                            h_flex()
                                .gap_0p5()
                                .child(gap())
                                .child(Self::hold_button(
                                    "down",
                                    IconName::ChevronDown,
                                    s(0.0, -1.0, 0.0),
                                    cx,
                                ))
                                .child(gap()),
                        ),
                )
            })
            .when(self.can_zoom, |this| {
                this.child(
                    v_flex()
                        .gap_0p5()
                        .child(Self::hold_button(
                            "zoom-in",
                            IconName::Plus,
                            s(0.0, 0.0, 1.0),
                            cx,
                        ))
                        .child(Self::hold_button(
                            "zoom-out",
                            IconName::Minus,
                            s(0.0, 0.0, -1.0),
                            cx,
                        )),
                )
            })
    }
}

fn pad_button(id: &'static str, icon: IconName) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(PAD_CELL))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(hsla(0., 0., 0., 0.5))
        .border_1()
        .border_color(hsla(0., 0., 1., 0.12))
        .text_color(hsla(0., 0., 1., 0.92))
        .cursor_pointer()
        .hover(|s| s.bg(hsla(0., 0., 0., 0.7)))
        .active(|s| s.bg(hsla(0., 0., 1., 0.28)))
        .child(Icon::new(icon).small())
}

type UsbCamera = camctl_uvc::Camera<camctl_usb::UsbTransport>;

/// What the controls start from.
#[derive(Debug)]
struct Initial {
    rig: Rig,
    can_aim: bool,
    can_zoom: bool,
    zoom_step: f64,
    vendor_id: u16,
    product_id: u16,
}

/// Try `read` a few times: a camera starting its stream may stall or time
/// out briefly.
fn with_retries<T, E>(read: impl Fn() -> Result<T, E>) -> Result<T, E> {
    const ATTEMPTS: usize = 4;
    let mut last = read();
    for _ in 1..ATTEMPTS {
        if last.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
        last = read();
    }
    last
}

/// Find, open and read the USB camera at `identity`. Blocking; call from a
/// background task.
fn open_usb_camera((location, vid, pid): (u32, u16, u16)) -> Option<(UsbCamera, Initial)> {
    let cameras = camctl_usb::list_cameras().ok()?;
    let info = cameras
        .iter()
        .find(|c| c.location_id == location && c.vendor_id == vid && c.product_id == pid)?;
    let camera = info.open().ok()?;
    let mut rig = Rig {
        pan: 0.0,
        tilt: 0.0,
        zoom: 0.0,
        pan_limits: Limits::new(0.0, 0.0),
        tilt_limits: Limits::new(0.0, 0.0),
        zoom_limits: Limits::new(0.0, 0.0),
        pan_sign: quirks::pan_sign(vid),
    };
    let mut can_aim = false;
    if camera.topology().supports(Control::PanTiltAbsolute)
        && let Ok(now) = with_retries(|| camera.get(Control::PanTiltAbsolute))
        && let Ok(range) = with_retries(|| camera.range(Control::PanTiltAbsolute))
        && let (Some(now), Some(lo), Some(hi)) = (
            now.pan_tilt_degrees(),
            range.min.and_then(|v| v.pan_tilt_degrees()),
            range.max.and_then(|v| v.pan_tilt_degrees()),
        )
        && let (Some(pan_limits), Some(tilt_limits)) = (
            Limits::from_range(lo.0, hi.0),
            Limits::from_range(lo.1, hi.1),
        )
    {
        rig.pan = now.0;
        rig.tilt = now.1;
        rig.pan_limits = pan_limits;
        rig.tilt_limits = tilt_limits;
        can_aim = true;
    }
    let mut zoom_step = 1.0;
    let mut can_zoom = false;
    if camera.topology().supports(Control::ZoomAbsolute)
        && let Ok(Value::Int(now)) = with_retries(|| camera.get(Control::ZoomAbsolute))
        && let Ok(range) = with_retries(|| camera.range(Control::ZoomAbsolute))
    {
        #[expect(clippy::cast_precision_loss, reason = "zoom units are small integers")]
        let (min, max, step, now) = (
            range.min.and_then(|v| v.as_int()).unwrap_or(0) as f64,
            range.max.and_then(|v| v.as_int()).unwrap_or(100) as f64,
            range.step.and_then(|v| v.as_int()).unwrap_or(1).max(1) as f64,
            now as f64,
        );
        if let Some(limits) = Limits::from_range(min, max) {
            rig.zoom_limits = limits;
            rig.zoom = now;
            zoom_step = step;
            can_zoom = true;
        }
    }
    Some((
        camera,
        Initial {
            rig,
            can_aim,
            can_zoom,
            zoom_step,
            vendor_id: vid,
            product_id: pid,
        },
    ))
}

impl Render for Preview {
    #[expect(
        clippy::too_many_lines,
        reason = "one declarative layout reads best whole"
    )]
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let live = self.status == Status::Live;
        let interactive = self.motion.is_some();

        let picture: AnyElement = match (&self.frame, self.message()) {
            (Some(frame), None) => surface(frame.pixel_buffer())
                .object_fit(ObjectFit::Cover)
                .size_full()
                .into_any_element(),
            (_, message) => {
                let (icon, title, detail) =
                    message.unwrap_or((IconName::Video, "Starting the camera", "".into()));
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(icon).size_8())
                    .child(div().text_color(theme.foreground).child(title))
                    .child(div().text_sm().max_w(px(320.)).text_center().child(detail))
                    .into_any_element()
            }
        };

        // Records where the video is (for click-to-aim) and listens for
        // releases and drag moves anywhere in the window (so a drag or pad
        // press always ends, even outside the video or with a modifier).
        let bounds_cell = self.video_bounds.clone();
        let weak = cx.weak_entity();
        let watcher = canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |_, (), window, _| {
                let up = weak.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = up.update(cx, |this, cx| this.on_release(event, window, cx));
                    }
                });
                let moved = weak.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = moved.update(cx, |this, cx| this.on_drag_move(event, window, cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let origin = self.video_bounds.get().origin;
        let video = div()
            .id("video")
            .group("video")
            .relative()
            .flex_1()
            .overflow_hidden()
            .when(interactive, |this| this.cursor(CursorStyle::Crosshair))
            .child(watcher)
            .child(picture)
            .children(self.marker.map(|at| {
                div()
                    .absolute()
                    .left(at.x - origin.x - px(14.))
                    .top(at.y - origin.y - px(14.))
                    .size(px(28.))
                    .rounded_full()
                    .border_2()
                    .border_color(hsla(0., 0., 1., 0.85))
            }))
            .when(interactive, |this| {
                this.child(
                    div()
                        .absolute()
                        .right_3()
                        .bottom_3()
                        .opacity(0.0)
                        .group_hover("video", |s| s.opacity(1.0))
                        .child(self.pad(cx)),
                )
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_video_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll));

        let bar = self.motion.as_ref().map(|m| {
            let r = *m.rig();
            h_flex()
                .h(px(36.))
                .px_3()
                .gap_3()
                .items_center()
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.background)
                .text_xs()
                .text_color(theme.muted_foreground)
                .children(self.zoom.as_ref().map(|zoom| {
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::ZoomIn).small())
                        .child(div().w(px(140.)).child(Slider::new(zoom)))
                        .child(format!("{:.1}×", m.zoom_factor()))
                }))
                .child(div().flex_1())
                .when(self.can_aim, |this| {
                    this.child(format!(
                        "{}  {}",
                        degrees(r.pan * r.pan_sign),
                        degrees(r.tilt)
                    ))
                })
                .child(
                    div()
                        .text_color(hsla(0., 0., 1., 0.35))
                        .child("drag to look · click to aim · scroll to zoom"),
                )
        });

        v_flex()
            .size_full()
            .bg(gpui_kit::black())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().size(px(7.)).rounded_full().bg(if live {
                            hsla(0.36, 0.7, 0.5, 1.)
                        } else {
                            theme.muted_foreground
                        }))
                        .child(self.device_name.clone()),
                ),
            )
            .child(video)
            .children(bar)
    }
}

#[cfg(test)]
mod tests {
    use super::degrees;

    #[test]
    fn degrees_never_show_minus_zero() {
        assert_eq!(degrees(-0.4), "0°");
        assert_eq!(degrees(0.0), "0°");
        assert_eq!(degrees(-1.6), "-2°");
        assert_eq!(degrees(12.5), "13°");
    }
}

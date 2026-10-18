//! Smooth camera motion from held input, and click-to-aim geometry.
//!
//! Cameras disagree about relative moves (the OBSBOT Tiny 2 Lite only pans
//! one way with them), but every PTZ camera follows absolute targets, and a
//! target that advances at a steady rate is followed smoothly by the motor.
//! So held input becomes a velocity, the velocity ramps up briefly, and each
//! tick advances absolute pan, tilt and zoom targets. Releasing stops the
//! targets at once; the motor's own lag eases the camera to rest.
//!
//! Everything here is in view terms: right, up and in are positive. The
//! camera's pan sign maps that onto its axis.
//!
//! Nothing here panics on bad input: non-finite values are ignored and
//! ranges are clamped without `f64::clamp` (which panics when inverted).

/// The range of one axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    min: f64,
    max: f64,
}

impl Limits {
    /// A range from trusted constants. An inverted range still clamps
    /// (to `min`) rather than panicking; use [`Limits::from_range`] for
    /// values read from a device.
    #[must_use]
    pub const fn new(min: f64, max: f64) -> Self {
        Limits { min, max }
    }

    /// A range from two device-reported ends, in either order. `None` when
    /// either end is not finite.
    #[must_use]
    pub fn from_range(a: f64, b: f64) -> Option<Self> {
        (a.is_finite() && b.is_finite()).then(|| Limits {
            min: a.min(b),
            max: a.max(b),
        })
    }

    #[must_use]
    pub const fn min(self) -> f64 {
        self.min
    }

    #[must_use]
    pub const fn max(self) -> f64 {
        self.max
    }

    /// `v` held inside the range. Never panics; NaN becomes `min`.
    #[must_use]
    pub fn clamp(self, v: f64) -> f64 {
        v.max(self.min).min(self.max)
    }

    fn span(self) -> f64 {
        self.max - self.min
    }
}

/// Where the camera points, how far it can go, and which way its pan
/// axis turns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rig {
    /// Degrees, in the camera's own sign.
    pub pan: f64,
    /// Degrees, positive is up.
    pub tilt: f64,
    /// The camera's zoom units (0..100 on the Tiny 2 Lite).
    pub zoom: f64,
    pub pan_limits: Limits,
    pub tilt_limits: Limits,
    pub zoom_limits: Limits,
    /// +1 if positive pan turns the camera right (the UVC convention),
    /// -1 if it turns left.
    pub pan_sign: f64,
}

/// Top speeds, and how long it takes to reach them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Speeds {
    /// Degrees per second.
    pub pan: f64,
    /// Degrees per second.
    pub tilt: f64,
    /// Zoom units per second.
    pub zoom: f64,
    /// Time from rest to top speed; a short ramp hides the start jolt.
    pub ramp_seconds: f64,
}

/// The lens: horizontal field of view at the widest zoom, and how far the
/// zoom range magnifies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fov {
    pub horizontal_degrees: f64,
    pub max_zoom_factor: f64,
}

impl Fov {
    /// 67.2 degrees horizontal (4:3) and 1x to 4x digital zoom, per
    /// OBSBOT's published specifications.
    pub const OBSBOT_TINY_2_LITE: Fov = Fov {
        horizontal_degrees: 67.2,
        max_zoom_factor: 4.0,
    };

    /// A guess for cameras camctl knows nothing about: a common webcam
    /// width, and no assumption about what the zoom range magnifies.
    pub const TYPICAL_WEBCAM: Fov = Fov {
        horizontal_degrees: 70.0,
        max_zoom_factor: 1.0,
    };
}

/// Held input, each axis -1..=1 as a fraction of top speed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stick {
    pub right: f64,
    pub up: f64,
    pub zoom_in: f64,
}

/// New targets to send after a tick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Targets {
    /// Pan and tilt in degrees, camera sign.
    pub pan_tilt: Option<(f64, f64)>,
    pub zoom: Option<f64>,
}

/// Turns held input and clicks into camera targets.
#[derive(Debug, Clone)]
pub struct Motion {
    rig: Rig,
    speeds: Speeds,
    fov: Fov,
    stick: Stick,
    /// Current speed per axis (right, up, in), in units per second.
    velocity: [f64; 3],
}

/// Changes smaller than this are not worth a USB write.
const EPSILON: f64 = 1e-6;

impl Motion {
    /// A motion model starting from `rig`, its position held inside its
    /// limits.
    #[must_use]
    pub fn new(rig: Rig, speeds: Speeds, fov: Fov) -> Self {
        let mut motion = Motion {
            rig,
            speeds,
            fov,
            stick: Stick::default(),
            velocity: [0.0; 3],
        };
        let r = motion.rig;
        motion.rig.pan = r.pan_limits.clamp(r.pan);
        motion.rig.tilt = r.tilt_limits.clamp(r.tilt);
        motion.rig.zoom = r.zoom_limits.clamp(r.zoom);
        motion
    }

    #[must_use]
    pub fn rig(&self) -> &Rig {
        &self.rig
    }

    /// Adopt positions read back from the camera (it may move on its own).
    /// Non-finite values are ignored.
    pub fn sync(&mut self, pan: f64, tilt: f64, zoom: f64) {
        if pan.is_finite() {
            self.rig.pan = self.rig.pan_limits.clamp(pan);
        }
        if tilt.is_finite() {
            self.rig.tilt = self.rig.tilt_limits.clamp(tilt);
        }
        if zoom.is_finite() {
            self.rig.zoom = self.rig.zoom_limits.clamp(zoom);
        }
    }

    /// Replace the held input. A zero axis stops that axis at once, and so
    /// does an axis that reverses, so a drag swung back turns at once.
    pub fn set_stick(&mut self, stick: Stick) {
        let unit = |v: f64| {
            if v.is_finite() {
                v.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        };
        self.stick = Stick {
            right: unit(stick.right),
            up: unit(stick.up),
            zoom_in: unit(stick.zoom_in),
        };
        let axes = self.stick_axes();
        for (v, s) in self.velocity.iter_mut().zip(axes) {
            if s == 0.0 || s.signum() != v.signum() {
                *v = 0.0;
            }
        }
    }

    #[must_use]
    pub fn stick(&self) -> Stick {
        self.stick
    }

    /// Whether anything is held, so ticks are still needed.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.stick_axes().iter().any(|&s| s != 0.0)
    }

    fn stick_axes(&self) -> [f64; 3] {
        [self.stick.right, self.stick.up, self.stick.zoom_in]
    }

    /// Advance by `dt` seconds. Returns the targets that moved.
    #[must_use]
    pub fn tick(&mut self, dt: f64) -> Option<Targets> {
        if !dt.is_finite() || dt <= 0.0 {
            return None;
        }
        let tops = [self.speeds.pan, self.speeds.tilt, self.speeds.zoom];
        let mut delta = [0.0; 3];
        for i in 0..3 {
            let goal = self.stick_axes()[i] * tops[i];
            let v = &mut self.velocity[i];
            if self.speeds.ramp_seconds <= 0.0 {
                *v = goal;
            } else {
                let step = tops[i] / self.speeds.ramp_seconds * dt;
                *v = if (goal - *v).abs() <= step {
                    goal
                } else {
                    *v + step.copysign(goal - *v)
                };
            }
            delta[i] = *v * dt;
        }
        let pan = self
            .rig
            .pan_limits
            .clamp(self.rig.pan + self.rig.pan_sign * delta[0]);
        let tilt = self.rig.tilt_limits.clamp(self.rig.tilt + delta[1]);
        let zoom = self.rig.zoom_limits.clamp(self.rig.zoom + delta[2]);
        let aimed = (pan - self.rig.pan).abs() > EPSILON || (tilt - self.rig.tilt).abs() > EPSILON;
        let zoomed = (zoom - self.rig.zoom).abs() > EPSILON;
        self.rig.pan = pan;
        self.rig.tilt = tilt;
        self.rig.zoom = zoom;
        (aimed || zoomed).then_some(Targets {
            pan_tilt: aimed.then_some((pan, tilt)),
            zoom: zoomed.then_some(zoom),
        })
    }

    /// Zoom by `delta` units (a scroll step). Returns the new zoom if it
    /// changed.
    #[must_use]
    pub fn zoom_by(&mut self, delta: f64) -> Option<f64> {
        if !delta.is_finite() {
            return None;
        }
        let zoom = self.rig.zoom_limits.clamp(self.rig.zoom + delta);
        if (zoom - self.rig.zoom).abs() <= EPSILON {
            return None;
        }
        self.rig.zoom = zoom;
        Some(zoom)
    }

    /// Magnification at the current zoom, 1.0 at the widest.
    #[must_use]
    pub fn zoom_factor(&self) -> f64 {
        let span = self.rig.zoom_limits.span();
        if span.is_nan() || span <= 0.0 {
            return 1.0;
        }
        let t = ((self.rig.zoom - self.rig.zoom_limits.min) / span).clamp(0.0, 1.0);
        (1.0 + (self.fov.max_zoom_factor - 1.0) * t).max(1.0)
    }

    /// Turn the camera so the point at (`x`, `y`) of the frame comes to the
    /// centre. `x` and `y` are offsets from the centre as fractions of the
    /// frame, -0.5..=0.5, with `y` positive downwards as on screen.
    /// `aspect` is the frame's width over height. Returns the new pan and
    /// tilt if they changed.
    ///
    /// The clicked point's ray is rotated by the current tilt before it is
    /// split into pan and tilt, so corners and tilted views aim truly, not
    /// just the centre lines of a level camera.
    #[must_use]
    pub fn aim_at(&mut self, x: f64, y: f64, aspect: f64) -> Option<(f64, f64)> {
        if !(x.is_finite() && y.is_finite() && aspect.is_finite() && aspect > 0.0) {
            return None;
        }
        let tan_h = (self.fov.horizontal_degrees / 2.0).to_radians().tan() / self.zoom_factor();
        let tan_v = tan_h / aspect;
        // The ray in camera space: right, up, forward.
        let (right, up) = (2.0 * x * tan_h, -2.0 * y * tan_v);
        // Rotate by the camera's tilt into pan-axis space.
        let (sin_t, cos_t) = self.rig.tilt.to_radians().sin_cos();
        let forward = cos_t - up * sin_t;
        let height = up * cos_t + sin_t;
        let turn_right = right.atan2(forward).to_degrees();
        let new_tilt = height.atan2(right.hypot(forward)).to_degrees();
        let pan = self
            .rig
            .pan_limits
            .clamp(self.rig.pan + self.rig.pan_sign * turn_right);
        let tilt = self.rig.tilt_limits.clamp(new_tilt);
        if (pan - self.rig.pan).abs() <= EPSILON && (tilt - self.rig.tilt).abs() <= EPSILON {
            return None;
        }
        self.rig.pan = pan;
        self.rig.tilt = tilt;
        Some((pan, tilt))
    }

    /// Centre the camera, zoom all the way out, and drop any held input.
    pub fn home(&mut self) {
        self.set_stick(Stick::default());
        self.rig.pan = self.rig.pan_limits.clamp(0.0);
        self.rig.tilt = self.rig.tilt_limits.clamp(0.0);
        self.rig.zoom = self.rig.zoom_limits.min;
    }

    /// Point the lens at the floor for sleep (see [`sleep_aim`]) and drop
    /// any held input. Zoom is left alone. Returns the pan and tilt it
    /// pointed at before, to aim back at on waking.
    pub fn park(&mut self, sleep_tilt: Option<f64>) -> (f64, f64) {
        self.set_stick(Stick::default());
        let was = (self.rig.pan, self.rig.tilt);
        (self.rig.pan, self.rig.tilt) =
            sleep_aim(self.rig.pan_limits, self.rig.tilt_limits, sleep_tilt);
        was
    }

    /// Point at `pan` and `tilt` degrees, camera sign, held inside the
    /// limits. Returns the new pan and tilt if they changed; non-finite
    /// input changes nothing.
    #[must_use]
    pub fn aim(&mut self, pan: f64, tilt: f64) -> Option<(f64, f64)> {
        if !(pan.is_finite() && tilt.is_finite()) {
            return None;
        }
        let pan = self.rig.pan_limits.clamp(pan);
        let tilt = self.rig.tilt_limits.clamp(tilt);
        if (pan - self.rig.pan).abs() <= EPSILON && (tilt - self.rig.tilt).abs() <= EPSILON {
            return None;
        }
        self.rig.pan = pan;
        self.rig.tilt = tilt;
        Some((pan, tilt))
    }
}

/// Where a sleeping camera points: pan centred and tilt as far down as it
/// goes, so the lens faces the desk or its own base. `sleep_tilt` replaces
/// the bottom of the tilt range for cameras that will not go there (see
/// [`crate::quirks::sleep_tilt`]). Pan and tilt are in degrees, camera
/// sign, each held inside its limits. A non-finite `sleep_tilt` is ignored.
#[must_use]
pub fn sleep_aim(pan_limits: Limits, tilt_limits: Limits, sleep_tilt: Option<f64>) -> (f64, f64) {
    let tilt = sleep_tilt
        .filter(|t| t.is_finite())
        .map_or(tilt_limits.min(), |t| tilt_limits.clamp(t));
    (pan_limits.clamp(0.0), tilt)
}

/// Where a point in a view shows up in a frame drawn with object-fit
/// cover (scaled to fill, centred, overflow cropped).
///
/// `view` and `frame` are (width, height); `point` is relative to the
/// view's top-left. Returns offsets from the frame's centre as fractions of
/// the frame, -0.5..=0.5, `y` positive downwards, or `None` for empty or
/// non-finite input.
#[must_use]
pub fn cover_offset(view: (f64, f64), frame: (f64, f64), point: (f64, f64)) -> Option<(f64, f64)> {
    let ((vw, vh), (fw, fh)) = (view, frame);
    let all = [vw, vh, fw, fh, point.0, point.1];
    if !all.iter().all(|v| v.is_finite()) || vw <= 0.0 || vh <= 0.0 || fw <= 0.0 || fh <= 0.0 {
        return None;
    }
    let scale = (vw / fw).max(vh / fh);
    let (shown_w, shown_h) = (fw * scale, fh * scale);
    let left = (vw - shown_w) / 2.0;
    let top = (vh - shown_h) / 2.0;
    Some((
        (point.0 - left) / shown_w - 0.5,
        (point.1 - top) / shown_h - 0.5,
    ))
}

/// Pixels a drag must travel before it moves the camera.
const DRAG_DEAD_ZONE: f64 = 4.0;

/// A joystick from a drag: `dx`, `dy` in pixels from where the drag began
/// (`dy` positive downwards), full speed at `radius` pixels. Speed grows
/// with the square of the distance past a small dead zone, so small drags
/// aim finely.
#[must_use]
pub fn drag_stick(dx: f64, dy: f64, radius: f64) -> Stick {
    let travel = radius - DRAG_DEAD_ZONE;
    let axis = |d: f64| {
        if !d.is_finite() || travel <= 0.0 {
            return 0.0;
        }
        let n = ((d.abs() - DRAG_DEAD_ZONE) / travel).clamp(0.0, 1.0);
        (n * n).copysign(d)
    };
    Stick {
        right: axis(dx),
        up: axis(-dy),
        zoom_in: 0.0,
    }
}

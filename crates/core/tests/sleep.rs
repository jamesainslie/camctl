//! Putting the camera to sleep: lens to the floor, and back again.

#![expect(clippy::float_cmp, reason = "clamped positions are exact by design")]

use camctl_core::motion::{Fov, Limits, Motion, Rig, Speeds, Stick, sleep_aim};

fn rig(pan: f64, tilt: f64) -> Rig {
    Rig {
        pan,
        tilt,
        zoom: 30.0,
        pan_limits: Limits::new(-130.0, 130.0),
        tilt_limits: Limits::new(-90.0, 90.0),
        zoom_limits: Limits::new(0.0, 100.0),
        pan_sign: -1.0,
    }
}

const SPEEDS: Speeds = Speeds {
    pan: 40.0,
    tilt: 40.0,
    zoom: 60.0,
    ramp_seconds: 0.25,
};

fn motion(pan: f64, tilt: f64) -> Motion {
    Motion::new(rig(pan, tilt), SPEEDS, Fov::OBSBOT_TINY_2_LITE)
}

#[test]
fn the_sleep_aim_centres_pan_and_tilts_all_the_way_down() {
    let aim = sleep_aim(Limits::new(-130.0, 130.0), Limits::new(-90.0, 90.0), None);
    assert_eq!(aim, (0.0, -90.0));
}

#[test]
fn the_sleep_aim_stays_inside_a_range_that_excludes_centre() {
    let aim = sleep_aim(Limits::new(10.0, 50.0), Limits::new(-30.0, 20.0), None);
    assert_eq!(aim, (10.0, -30.0));
}

#[test]
fn parking_points_the_lens_down_and_returns_the_aim_it_left() {
    let mut m = motion(20.0, -11.0);
    let was = m.park(None);
    assert_eq!(was, (20.0, -11.0));
    let r = m.rig();
    assert_eq!((r.pan, r.tilt), (0.0, -90.0));
    // Zoom is left alone, so waking restores the same framing.
    assert_eq!(r.zoom, 30.0);
}

#[test]
fn a_camera_specific_sleep_tilt_replaces_the_bottom_of_the_range() {
    let (pan, tilt) = (Limits::new(-130.0, 130.0), Limits::new(-90.0, 90.0));
    assert_eq!(sleep_aim(pan, tilt, Some(-80.0)), (0.0, -80.0));
    // Still held inside the range.
    assert_eq!(sleep_aim(pan, tilt, Some(-200.0)), (0.0, -90.0));
    assert_eq!(sleep_aim(pan, tilt, Some(f64::NAN)), (0.0, -90.0));
}

#[test]
fn parking_uses_a_camera_specific_sleep_tilt() {
    let mut m = motion(20.0, -11.0);
    assert_eq!(m.park(Some(-80.0)), (20.0, -11.0));
    let r = m.rig();
    assert_eq!((r.pan, r.tilt), (0.0, -80.0));
}

#[test]
fn parking_drops_held_input() {
    let mut m = motion(0.0, 0.0);
    m.set_stick(Stick {
        right: 1.0,
        up: 1.0,
        zoom_in: 1.0,
    });
    let _ = m.park(None);
    assert!(!m.is_active());
    assert_eq!(m.tick(0.1), None);
}

#[test]
fn aiming_back_restores_the_parked_pose() {
    let mut m = motion(20.0, -11.0);
    let was = m.park(None);
    assert_eq!(m.aim(was.0, was.1), Some((20.0, -11.0)));
    let r = m.rig();
    assert_eq!((r.pan, r.tilt), (20.0, -11.0));
}

#[test]
fn aiming_clamps_to_the_limits_and_reports_no_change_when_already_there() {
    let mut m = motion(0.0, 0.0);
    assert_eq!(m.aim(500.0, -500.0), Some((130.0, -90.0)));
    assert_eq!(m.aim(130.0, -90.0), None);
}

#[test]
fn aiming_ignores_non_finite_input() {
    let mut m = motion(5.0, 5.0);
    assert_eq!(m.aim(f64::NAN, f64::INFINITY), None);
    let r = m.rig();
    assert_eq!((r.pan, r.tilt), (5.0, 5.0));
}

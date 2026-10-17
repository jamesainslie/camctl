//! Edge cases of the motion model: real aiming geometry, hostile input,
//! and bad ranges from a camera.

use camctl_core::motion::{Fov, Limits, Motion, Rig, Speeds, Stick, cover_offset, drag_stick};

fn rig(pan_sign: f64, tilt: f64) -> Rig {
    Rig {
        pan: 0.0,
        tilt,
        zoom: 0.0,
        pan_limits: Limits::new(-130.0, 130.0),
        tilt_limits: Limits::new(-90.0, 90.0),
        zoom_limits: Limits::new(0.0, 100.0),
        pan_sign,
    }
}

const SPEEDS: Speeds = Speeds {
    pan: 40.0,
    tilt: 40.0,
    zoom: 60.0,
    ramp_seconds: 0.25,
};

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

#[test]
fn a_corner_click_tilts_by_the_true_angle_not_the_vertical_half_field() {
    let mut m = Motion::new(rig(1.0, 0.0), SPEEDS, Fov::OBSBOT_TINY_2_LITE);
    let (pan, tilt) = m.aim_at(0.5, -0.5, 16.0 / 9.0).unwrap();
    assert!(close(pan, 33.6, 0.05), "pan {pan}");
    assert!(close(tilt, 17.29, 0.05), "tilt {tilt}");
}

#[test]
fn an_edge_click_with_the_camera_tilted_pans_further_and_lowers_the_tilt() {
    let mut m = Motion::new(rig(1.0, 30.0), SPEEDS, Fov::OBSBOT_TINY_2_LITE);
    let (pan, tilt) = m.aim_at(0.5, 0.0, 16.0 / 9.0).unwrap();
    assert!(close(pan, 37.5, 0.1), "pan {pan}");
    assert!(close(tilt, 24.6, 0.1), "tilt {tilt}");
}

#[test]
fn uvc_signed_cameras_pan_right_for_positive_values() {
    let mut m = Motion::new(rig(1.0, 0.0), SPEEDS, Fov::OBSBOT_TINY_2_LITE);
    m.set_stick(Stick {
        right: 1.0,
        ..Stick::default()
    });
    for _ in 0..30 {
        let _ = m.tick(1.0 / 30.0);
    }
    assert!(m.rig().pan > 30.0);
}

#[test]
fn reversing_a_drag_reverses_at_once() {
    let mut m = Motion::new(rig(1.0, 0.0), SPEEDS, Fov::OBSBOT_TINY_2_LITE);
    m.set_stick(Stick {
        right: 1.0,
        ..Stick::default()
    });
    for _ in 0..30 {
        let _ = m.tick(1.0 / 30.0);
    }
    let before = m.rig().pan;
    m.set_stick(Stick {
        right: -1.0,
        ..Stick::default()
    });
    let _ = m.tick(1.0 / 30.0);
    assert!(m.rig().pan <= before, "no carrying on the old way");
}

#[test]
fn infinite_or_nan_time_steps_are_ignored() {
    let mut m = Motion::new(rig(1.0, 0.0), SPEEDS, Fov::OBSBOT_TINY_2_LITE);
    m.set_stick(Stick {
        right: 1.0,
        ..Stick::default()
    });
    assert!(m.tick(f64::INFINITY).is_none());
    assert!(m.tick(f64::NAN).is_none());
    assert!(m.rig().pan.is_finite() && m.rig().tilt.is_finite());
}

#[test]
fn non_finite_sync_values_are_ignored() {
    let mut m = Motion::new(rig(1.0, 0.0), SPEEDS, Fov::OBSBOT_TINY_2_LITE);
    m.sync(f64::NAN, 10.0, f64::INFINITY);
    let r = m.rig();
    assert!(close(r.pan, 0.0, 0.0) && close(r.tilt, 10.0, 0.0) && close(r.zoom, 0.0, 0.0));
}

#[test]
fn bad_ranges_from_a_camera_are_sorted_or_refused() {
    let l = Limits::from_range(90.0, -90.0).unwrap();
    assert!(close(l.min(), -90.0, 0.0) && close(l.max(), 90.0, 0.0));
    assert!(Limits::from_range(f64::NAN, 1.0).is_none());
    assert!(Limits::from_range(0.0, f64::INFINITY).is_none());
}

#[test]
fn an_inverted_range_never_panics() {
    let mut r = rig(1.0, 0.0);
    r.tilt_limits = Limits::new(10.0, -10.0);
    let mut m = Motion::new(r, SPEEDS, Fov::OBSBOT_TINY_2_LITE);
    m.set_stick(Stick {
        up: 1.0,
        ..Stick::default()
    });
    let _ = m.tick(1.0 / 30.0);
    let _ = m.aim_at(0.2, 0.2, 16.0 / 9.0);
}

#[test]
fn a_zoom_outside_its_range_does_not_flip_the_aim() {
    let mut r = rig(1.0, 0.0);
    r.zoom = -50.0;
    let mut m = Motion::new(r, SPEEDS, Fov::OBSBOT_TINY_2_LITE);
    assert!(m.zoom_factor() >= 1.0);
    let (pan, _) = m.aim_at(0.5, 0.0, 16.0 / 9.0).unwrap();
    assert!(pan > 0.0);
}

#[test]
fn non_finite_sizes_give_nothing() {
    assert!(cover_offset((f64::NAN, 300.0), (640.0, 360.0), (1.0, 1.0)).is_none());
    assert!(cover_offset((800.0, 300.0), (640.0, f64::INFINITY), (1.0, 1.0)).is_none());
    assert!(cover_offset((800.0, 300.0), (640.0, 360.0), (f64::NAN, 1.0)).is_none());
}

#[test]
fn full_drag_speed_is_reached_at_the_radius() {
    let s = drag_stick(100.0, 0.0, 100.0);
    assert!(close(s.right, 1.0, 1e-9), "right {}", s.right);
}

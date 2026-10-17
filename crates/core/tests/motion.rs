#![expect(
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped positions are exact by design, and the tick counts are small"
)]

use camctl_core::motion::{Fov, Limits, Motion, Rig, Speeds, Stick};

fn rig() -> Rig {
    Rig {
        pan: 0.0,
        tilt: 0.0,
        zoom: 0.0,
        pan_limits: Limits::new(-130.0, 130.0),
        tilt_limits: Limits::new(-90.0, 90.0),
        zoom_limits: Limits::new(0.0, 100.0),
        // The OBSBOT Tiny 2 Lite turns left for positive pan.
        pan_sign: -1.0,
    }
}

fn speeds() -> Speeds {
    Speeds {
        pan: 40.0,
        tilt: 40.0,
        zoom: 60.0,
        ramp_seconds: 0.25,
    }
}

fn motion() -> Motion {
    Motion::new(rig(), speeds(), Fov::OBSBOT_TINY_2_LITE)
}

/// Run `seconds` of 30 Hz ticks.
fn run(m: &mut Motion, seconds: f64) {
    let dt = 1.0 / 30.0;
    for _ in 0..(seconds * 30.0).round() as usize {
        let _ = m.tick(dt);
    }
}

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

#[test]
fn holding_right_turns_the_camera_right_after_a_ramp() {
    let mut m = motion();
    m.set_stick(Stick {
        right: 1.0,
        ..Stick::default()
    });
    run(&mut m, 1.0);
    // 1 s at 40 deg/s minus half the 0.25 s ramp: about 35 deg, and right
    // is negative pan on this camera.
    let pan = m.rig().pan;
    assert!(close(pan, -35.0, 1.5), "pan {pan}");
    assert_eq!(m.rig().tilt, 0.0);
}

#[test]
fn up_and_zoom_in_are_positive() {
    let mut m = motion();
    m.set_stick(Stick {
        up: 0.5,
        zoom_in: 1.0,
        ..Stick::default()
    });
    run(&mut m, 1.0);
    assert!(m.rig().tilt > 10.0);
    assert!(m.rig().zoom > 40.0);
}

#[test]
fn releasing_stops_at_once() {
    let mut m = motion();
    m.set_stick(Stick {
        right: 1.0,
        ..Stick::default()
    });
    run(&mut m, 0.5);
    m.set_stick(Stick::default());
    let at_release = m.rig().pan;
    assert!(m.tick(1.0 / 30.0).is_none(), "no target after release");
    assert!(!m.is_active());
    assert_eq!(m.rig().pan, at_release);
}

#[test]
fn ticks_report_only_what_changed() {
    let mut m = motion();
    m.set_stick(Stick {
        zoom_in: 1.0,
        ..Stick::default()
    });
    run(&mut m, 0.1);
    let t = m.tick(1.0 / 30.0).expect("zooming");
    assert!(t.zoom.is_some());
    assert!(t.pan_tilt.is_none());
}

#[test]
fn motion_stops_at_the_limits() {
    let mut m = motion();
    m.set_stick(Stick {
        up: 1.0,
        ..Stick::default()
    });
    run(&mut m, 5.0);
    assert_eq!(m.rig().tilt, 90.0);
    // Pinned at the limit: nothing new to send.
    assert!(m.tick(1.0 / 30.0).is_none());
}

#[test]
fn zoom_steps_are_clamped() {
    let mut m = motion();
    assert_eq!(m.zoom_by(30.0), Some(30.0));
    assert_eq!(m.zoom_by(500.0), Some(100.0));
    assert_eq!(m.zoom_by(10.0), None, "already at the limit");
    assert_eq!(m.zoom_by(-1000.0), Some(0.0));
}

#[test]
fn clicking_the_centre_does_nothing() {
    let mut m = motion();
    assert_eq!(m.aim_at(0.0, 0.0, 16.0 / 9.0), None);
}

#[test]
fn clicking_the_right_edge_turns_half_the_field_of_view() {
    let mut m = motion();
    let (pan, tilt) = m.aim_at(0.5, 0.0, 16.0 / 9.0).unwrap();
    // Half of the 67.2 deg horizontal field of view, to the right.
    assert!(close(pan, -33.6, 0.05), "pan {pan}");
    assert!(close(tilt, 0.0, 1e-9));
}

#[test]
fn clicking_near_the_top_tilts_up_by_the_vertical_field() {
    let mut m = motion();
    let (_, tilt) = m.aim_at(0.0, -0.5, 16.0 / 9.0).unwrap();
    // tan(v/2) = tan(33.6 deg) * 9/16.
    let want = (33.6f64.to_radians().tan() * 9.0 / 16.0)
        .atan()
        .to_degrees();
    assert!(close(tilt, want, 0.05), "tilt {tilt} want {want}");
}

#[test]
fn zooming_in_narrows_the_aim() {
    let mut m = motion();
    let _ = m.zoom_by(100.0); // 4x
    let (pan, _) = m.aim_at(0.5, 0.0, 16.0 / 9.0).unwrap();
    let want = -(33.6f64.to_radians().tan() / 4.0).atan().to_degrees();
    assert!(close(pan, want, 0.05), "pan {pan} want {want}");
}

#[test]
fn aiming_moves_from_where_the_camera_points_and_clamps() {
    let mut m = motion();
    let _ = m.aim_at(0.5, 0.0, 16.0 / 9.0);
    let _ = m.aim_at(0.5, 0.0, 16.0 / 9.0);
    let _ = m.aim_at(0.5, 0.0, 16.0 / 9.0);
    let _ = m.aim_at(0.5, 0.0, 16.0 / 9.0);
    let _ = m.aim_at(0.5, 0.0, 16.0 / 9.0);
    assert_eq!(m.rig().pan, -130.0);
}

#[test]
fn home_centres_and_zooms_out() {
    let mut m = motion();
    let _ = m.zoom_by(50.0);
    let _ = m.aim_at(0.3, 0.2, 16.0 / 9.0);
    m.home();
    let r = m.rig();
    assert_eq!((r.pan, r.tilt, r.zoom), (0.0, 0.0, 0.0));
}

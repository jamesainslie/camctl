#![expect(clippy::float_cmp, reason = "signs and lens constants compare exactly")]

use camctl_core::motion::Fov;
use camctl_core::quirks;

const OBSBOT: u16 = 0x3564;
const TINY_2_LITE: u16 = 0xfef9;

#[test]
fn obsbot_cameras_pan_left_for_positive_values() {
    assert_eq!(quirks::pan_sign(OBSBOT), -1.0);
}

#[test]
fn other_cameras_follow_the_uvc_convention() {
    assert_eq!(quirks::pan_sign(0x046d), 1.0); // Logitech
}

#[test]
fn the_tiny_2_lite_lens_is_known() {
    assert_eq!(quirks::fov(OBSBOT, TINY_2_LITE), Fov::OBSBOT_TINY_2_LITE);
}

#[test]
fn unknown_cameras_get_a_typical_webcam_lens_without_zoom_magnification() {
    let fov = quirks::fov(0x046d, 0x085e);
    assert_eq!(fov, Fov::TYPICAL_WEBCAM);
    assert_eq!(fov.max_zoom_factor, 1.0);
}

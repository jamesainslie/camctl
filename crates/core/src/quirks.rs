//! What individual cameras do differently from the UVC specification.

use crate::motion::Fov;

/// OBSBOT Tech's USB vendor ID.
const OBSBOT: u16 = 0x3564;
/// OBSBOT Tiny 2 Lite.
const OBSBOT_TINY_2_LITE: u16 = 0xfef9;

/// +1 if positive pan turns the camera right (the UVC convention), -1 if
/// it turns left. OBSBOT cameras turn left.
#[must_use]
pub fn pan_sign(vendor_id: u16) -> f64 {
    if vendor_id == OBSBOT { -1.0 } else { 1.0 }
}

/// The tilt, in degrees, that puts the camera to sleep, when the bottom of
/// its reported range will not do. The Tiny 2 Lite reports -90 degrees but
/// ignores tilts below about -84; sent to -80 it tips the rest of the way
/// down by itself and enters its own sleep, ignoring pan and tilt until an
/// app opens its video. `None` means the bottom of the range.
#[must_use]
pub fn sleep_tilt(vendor_id: u16, product_id: u16) -> Option<f64> {
    match (vendor_id, product_id) {
        (OBSBOT, OBSBOT_TINY_2_LITE) => Some(-80.0),
        _ => None,
    }
}

/// The lens, for click-to-aim. Unknown cameras get a typical webcam
/// guess, so aiming lands close and a second click refines it.
#[must_use]
pub fn fov(vendor_id: u16, product_id: u16) -> Fov {
    match (vendor_id, product_id) {
        (OBSBOT, OBSBOT_TINY_2_LITE) => Fov::OBSBOT_TINY_2_LITE,
        _ => Fov::TYPICAL_WEBCAM,
    }
}

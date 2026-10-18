//! Camera capture through AVFoundation, delivering frames as IOSurface
//! backed 420f `CVPixelBuffer`s, the format GPUI's `surface` element draws
//! without copying.

pub mod policy;
mod session;
mod slot;

pub use session::{
    Authorization, CaptureError, Device, DeviceKind, Frame, Session, authorization, devices,
    encoding_of, request_access,
};

/// The USB identity encoded in a UVC camera's AVFoundation unique ID:
/// `0x` + location ID + VID (4 hex digits) + PID (4 hex digits), e.g.
/// `0x1224003564fef9` is location 0x00122400, VID 0x3564, PID 0xfef9.
/// `None` for anything else (built-in cameras use a UUID).
#[must_use]
pub fn usb_identity(unique_id: &str) -> Option<(u32, u16, u16)> {
    let hex = unique_id.strip_prefix("0x")?;
    if hex.len() <= 8 || hex.len() > 16 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (location, ids) = hex.split_at(hex.len() - 8);
    Some((
        u32::from_str_radix(location, 16).ok()?,
        u16::from_str_radix(&ids[..4], 16).ok()?,
        u16::from_str_radix(&ids[4..], 16).ok()?,
    ))
}

/// Which of the cameras, given by unique ID in [`devices`] order, to
/// preview: `preferred` if it is connected, otherwise the first USB camera,
/// otherwise the first camera of any kind. `None` with no cameras.
#[must_use]
pub fn pick_camera(unique_ids: &[&str], preferred: Option<&str>) -> Option<usize> {
    preferred
        .and_then(|want| unique_ids.iter().position(|id| *id == want))
        .or_else(|| unique_ids.iter().position(|id| usb_identity(id).is_some()))
        .or_else(|| (!unique_ids.is_empty()).then_some(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uvc_unique_ids_carry_location_vid_and_pid() {
        assert_eq!(
            usb_identity("0x1224003564fef9"),
            Some((0x0012_2400, 0x3564, 0xfef9))
        );
    }

    #[test]
    fn other_unique_ids_are_not_usb() {
        assert_eq!(usb_identity("6C707041-05AC-0010-0008-000000000001"), None);
        assert_eq!(usb_identity("0x3564fef9"), None);
        assert_eq!(usb_identity("0xzz24003564fef9"), None);
        assert_eq!(usb_identity(""), None);
    }

    const BUILT_IN: &str = "6C707041-05AC-0010-0008-000000000001";
    const OBSBOT: &str = "0x1224003564fef9";
    const OTHER_USB: &str = "0x1100000046d085e";

    #[test]
    fn the_preferred_camera_wins_when_connected() {
        let ids = [BUILT_IN, OBSBOT, OTHER_USB];
        assert_eq!(pick_camera(&ids, Some(OTHER_USB)), Some(2));
        assert_eq!(pick_camera(&ids, Some(BUILT_IN)), Some(0));
    }

    #[test]
    fn without_a_preference_the_first_usb_camera_wins() {
        assert_eq!(pick_camera(&[BUILT_IN, OBSBOT, OTHER_USB], None), Some(1));
    }

    #[test]
    fn a_missing_preferred_camera_falls_back_to_the_default() {
        assert_eq!(pick_camera(&[BUILT_IN, OBSBOT], Some(OTHER_USB)), Some(1));
    }

    #[test]
    fn with_no_usb_camera_the_first_camera_wins() {
        assert_eq!(pick_camera(&[BUILT_IN], None), Some(0));
        assert_eq!(pick_camera(&[], Some(OBSBOT)), None);
    }

    #[test]
    fn usb_cameras_report_decoded_mjpeg_as_420v() {
        // Seen on an OBSBOT Tiny 2 Lite: its YUY2 modes arrive as `yuvs`,
        // its MJPEG modes as `420v` after macOS decodes them.
        use policy::Encoding::{Mjpeg, Other, Uncompressed};
        let code = |s: &[u8; 4]| u32::from_be_bytes(*s);
        assert_eq!(encoding_of(code(b"yuvs"), DeviceKind::Usb), Uncompressed);
        assert_eq!(encoding_of(code(b"420v"), DeviceKind::Usb), Mjpeg);
        assert_eq!(encoding_of(code(b"dmb1"), DeviceKind::Usb), Mjpeg);
        assert_eq!(encoding_of(code(b"avc1"), DeviceKind::Usb), Other);
    }

    #[test]
    fn built_in_cameras_produce_420v_natively() {
        let code = |s: &[u8; 4]| u32::from_be_bytes(*s);
        assert_eq!(
            encoding_of(code(b"420v"), DeviceKind::BuiltIn),
            policy::Encoding::Uncompressed
        );
    }
}

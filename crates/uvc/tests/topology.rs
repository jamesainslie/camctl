//! Topology edge cases beyond the real fixture: several video functions,
//! junk in unrelated interfaces, frame-based formats, continuous intervals.

use camctl_uvc::{Control, FormatKind, FrameIntervals, ParseError, parse_configuration};

const TINY2_LITE: &[u8] = include_bytes!("fixtures/obsbot-tiny-2-lite-config.bin");

/// Set wTotalLength to the buffer's real length.
fn fix_total(bytes: &mut [u8]) {
    let total = u16::try_from(bytes.len()).expect("fixtures fit in u16");
    bytes[2..4].copy_from_slice(&total.to_le_bytes());
}

/// The fixture plus `extra`, with wTotalLength updated to match.
fn fixture_with(extra: &[u8]) -> Vec<u8> {
    let mut bytes = TINY2_LITE.to_vec();
    bytes.extend_from_slice(extra);
    fix_total(&mut bytes);
    bytes
}

#[test]
fn a_second_video_function_is_ignored() {
    // VC interface 4, its UVC 1.1 header, and a camera terminal (id 9)
    // that advertises only Privacy.
    let second = [
        9, 0x04, 4, 0, 0, 0x0e, 0x01, 0, 0, //
        13, 0x24, 0x01, 0x10, 0x01, 0, 0, 0, 0, 0, 0, 1, 5, //
        18, 0x24, 0x02, 9, 0x01, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0x04,
    ];
    let t = parse_configuration(&fixture_with(&second)).unwrap();
    assert_eq!(t.uvc_version, 0x0100);
    assert_eq!(t.camera_terminal.as_ref().unwrap().id, 1);
    assert!(t.supports(Control::ZoomAbsolute));
    assert!(!t.supports(Control::Privacy));
    assert_eq!(t.formats.len(), 2);
}

#[test]
fn a_bad_descriptor_in_an_unrelated_interface_is_skipped() {
    // The fixture ends in its audio interfaces; a stub class-specific
    // descriptor there must not reject the camera.
    let t = parse_configuration(&fixture_with(&[2, 0x24])).unwrap();
    assert!(t.supports(Control::ZoomAbsolute));
}

#[test]
fn formats_record_their_streaming_interface() {
    let t = parse_configuration(TINY2_LITE).unwrap();
    assert!(t.formats.iter().all(|f| f.interface == 1));
}

/// A minimal UVC 1.1 camera with one frame-based H.264 format whose only
/// frame has a continuous interval range.
fn frame_based_camera() -> Vec<u8> {
    let mut b = vec![9, 0x02, 0, 0, 2, 1, 0, 0x80, 50];
    b.extend_from_slice(&[9, 0x04, 0, 0, 0, 0x0e, 0x01, 0, 0]);
    b.extend_from_slice(&[13, 0x24, 0x01, 0x10, 0x01, 0, 0, 0, 0, 0, 0, 1, 1]);
    b.extend_from_slice(&[9, 0x04, 1, 0, 0, 0x0e, 0x02, 0, 0]);
    let mut format = vec![28, 0x24, 0x10, 1, 1];
    format.extend_from_slice(b"H264");
    format.extend_from_slice(&[0; 12]);
    format.extend_from_slice(&[16, 1, 0, 0, 0, 0, 1]);
    b.extend_from_slice(&format);
    let mut frame = vec![38, 0x24, 0x11, 1, 0];
    frame.extend_from_slice(&1920u16.to_le_bytes());
    frame.extend_from_slice(&1080u16.to_le_bytes());
    frame.extend_from_slice(&[0; 8]);
    frame.extend_from_slice(&333_333u32.to_le_bytes());
    frame.push(0);
    frame.extend_from_slice(&[0; 4]);
    for v in [166_666u32, 10_000_000, 1] {
        frame.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&frame);
    fix_total(&mut b);
    b
}

#[test]
fn frame_based_formats_with_continuous_intervals() {
    let t = parse_configuration(&frame_based_camera()).unwrap();
    assert_eq!(t.uvc_version, 0x0110);
    let f = &t.formats[0];
    assert_eq!(f.kind, FormatKind::FrameBased(*b"H264"));
    assert_eq!(f.interface, 1);
    let frame = &f.frames[0];
    assert_eq!(
        (frame.width, frame.height, frame.default_interval),
        (1920, 1080, 333_333)
    );
    assert_eq!(
        frame.intervals,
        FrameIntervals::Continuous {
            min: 166_666,
            max: 10_000_000,
            step: 1
        }
    );
    assert_eq!(frame.fps(), vec![60.0, 1.0]);
}

#[test]
fn a_streaming_interface_outside_the_collection_is_ignored() {
    let mut b = frame_based_camera();
    // The header's baInterfaceNr names interface 1; renumber the streaming
    // interface to 2.
    let vs = b
        .windows(7)
        .position(|w| w == [9, 0x04, 1, 0, 0, 0x0e, 0x02])
        .unwrap();
    b[vs + 2] = 2;
    let t = parse_configuration(&b).unwrap();
    assert!(t.formats.is_empty());
}

#[test]
fn a_video_control_interface_without_a_header_is_an_error() {
    let mut b = vec![
        9, 0x02, 0, 0, 1, 1, 0, 0x80, 50, 9, 0x04, 0, 0, 0, 0x0e, 0x01, 0, 0,
    ];
    fix_total(&mut b);
    assert!(matches!(
        parse_configuration(&b),
        Err(ParseError::MissingHeader)
    ));
}

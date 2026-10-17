use camctl_uvc::{Control, FormatKind, ParseError, parse_configuration};

const TINY2_LITE: &[u8] = include_bytes!("fixtures/obsbot-tiny-2-lite-config.bin");

#[test]
fn finds_video_control_interface_and_version() {
    let t = parse_configuration(TINY2_LITE).unwrap();
    assert_eq!(t.control_interface, 0);
    assert_eq!(t.uvc_version, 0x0100);
}

#[test]
fn camera_terminal_advertises_ptz_exposure_and_focus() {
    let t = parse_configuration(TINY2_LITE).unwrap();
    let ct = t.camera_terminal.as_ref().expect("camera terminal");
    assert_eq!(ct.id, 1);
    for c in [
        Control::AeMode,
        Control::ExposureTimeAbsolute,
        Control::ExposureTimeRelative,
        Control::FocusAbsolute,
        Control::FocusAuto,
        Control::ZoomAbsolute,
        Control::ZoomRelative,
        Control::PanTiltAbsolute,
        Control::PanTiltRelative,
        Control::RollAbsolute,
    ] {
        assert!(t.supports(c), "{c:?} should be supported");
    }
    for c in [
        Control::ScanningMode,
        Control::AePriority,
        Control::IrisAbsolute,
        Control::RollRelative,
        Control::Privacy,
    ] {
        assert!(!t.supports(c), "{c:?} should not be supported");
    }
}

#[test]
fn processing_unit_advertises_image_controls() {
    let t = parse_configuration(TINY2_LITE).unwrap();
    let pu = &t.processing_units[0];
    assert_eq!((pu.id, pu.source_id, pu.max_multiplier), (3, 1, 400));
    for c in [
        Control::Brightness,
        Control::Contrast,
        Control::Hue,
        Control::Saturation,
        Control::Sharpness,
        Control::WhiteBalanceTemperature,
        Control::WhiteBalanceTemperatureAuto,
        Control::WhiteBalanceComponent,
        Control::WhiteBalanceComponentAuto,
        Control::BacklightCompensation,
        Control::Gain,
        Control::PowerLineFrequency,
        Control::DigitalMultiplier,
        Control::DigitalMultiplierLimit,
    ] {
        assert!(t.supports(c), "{c:?} should be supported");
    }
    assert!(!t.supports(Control::Gamma));
    assert!(!t.supports(Control::HueAuto));
}

#[test]
fn extension_unit_is_listed_with_its_guid() {
    let t = parse_configuration(TINY2_LITE).unwrap();
    assert_eq!(t.extension_units.len(), 1);
    let xu = &t.extension_units[0];
    assert_eq!(xu.id, 2);
    assert_eq!(xu.num_controls, 19);
    assert_eq!(
        xu.guid,
        [
            0x91, 0x72, 0x1e, 0x9a, 0x43, 0x68, 0x83, 0x46, 0x6d, 0x92, 0x39, 0xbc, 0x79, 0x06,
            0xee, 0x49
        ]
    );
}

#[test]
fn control_addresses_route_to_their_unit() {
    let t = parse_configuration(TINY2_LITE).unwrap();
    let zoom = t.address(Control::ZoomAbsolute).unwrap();
    assert_eq!((zoom.unit_id, zoom.interface, zoom.selector), (1, 0, 0x0b));
    let gain = t.address(Control::Gain).unwrap();
    assert_eq!((gain.unit_id, gain.interface, gain.selector), (3, 0, 0x04));
    assert!(t.address(Control::Privacy).is_none());
}

#[test]
fn streaming_formats_and_frames() {
    let t = parse_configuration(TINY2_LITE).unwrap();
    assert_eq!(t.formats.len(), 2);

    let mjpeg = &t.formats[0];
    assert_eq!(mjpeg.index, 1);
    assert_eq!(mjpeg.kind, FormatKind::Mjpeg);
    let sizes: Vec<_> = mjpeg.frames.iter().map(|f| (f.width, f.height)).collect();
    assert_eq!(sizes, [(1920, 1080), (3840, 2160), (1280, 720)]);
    assert_eq!(mjpeg.frames[0].max_fps(), Some(60.0));
    assert_eq!(mjpeg.frames[1].max_fps(), Some(30.0));

    let yuy2 = &t.formats[1];
    assert_eq!(yuy2.index, 2);
    assert_eq!(yuy2.kind, FormatKind::Uncompressed(*b"YUY2"));
    let sizes: Vec<_> = yuy2.frames.iter().map(|f| (f.width, f.height)).collect();
    assert_eq!(sizes, [(640, 360), (640, 480)]);
    assert_eq!(yuy2.frames[0].fps(), vec![30.0, 25.0, 24.0, 20.0, 15.0]);
}

#[test]
fn truncated_input_is_an_error() {
    let cut = &TINY2_LITE[..TINY2_LITE.len() - 7];
    assert!(matches!(
        parse_configuration(cut),
        Err(ParseError::Truncated { .. })
    ));
}

#[test]
fn zero_length_descriptor_is_an_error_not_a_hang() {
    let mut bytes = TINY2_LITE[..9].to_vec();
    bytes.extend_from_slice(&[0, 0x24, 0x01]);
    // Declare the real length, so only the zero-length descriptor is wrong.
    bytes[2..4].copy_from_slice(&12u16.to_le_bytes());
    assert!(matches!(
        parse_configuration(&bytes),
        Err(ParseError::ZeroLength { .. })
    ));
}

#[test]
fn non_uvc_configuration_is_an_error() {
    // A configuration descriptor alone, with no interfaces.
    let bytes = [9, 2, 9, 0, 0, 1, 0, 0x80, 50];
    assert!(matches!(
        parse_configuration(&bytes),
        Err(ParseError::NoVideoControl)
    ));
}

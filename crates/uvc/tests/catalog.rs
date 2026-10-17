use std::collections::HashSet;

use camctl_uvc::{Control, Shape, Value};

#[test]
fn the_catalog_is_complete_sorted_and_unambiguous() {
    let all = Control::ALL;
    assert_eq!(all.len(), Control::HueAuto as usize + 1);
    assert!(
        all.windows(2).all(|w| w[0] < w[1]),
        "ALL is sorted and unique"
    );
    let mut selectors = HashSet::new();
    let mut bits = HashSet::new();
    for c in all {
        let s = c.spec();
        assert!(
            selectors.insert((s.unit, s.selector)),
            "{c:?} selector clash"
        );
        assert!(bits.insert((s.unit, s.bit)), "{c:?} bit clash");
    }
}

#[test]
fn controls_display_their_names() {
    assert_eq!(Control::ZoomAbsolute.to_string(), "Zoom");
}

/// A representative valid value for each shape.
fn sample(shape: Shape) -> Value {
    match shape {
        Shape::Scalar { signed: true, .. } => Value::Int(-7),
        Shape::Scalar { .. } | Shape::Enum => Value::Int(7),
        Shape::Bool => Value::Int(1),
        Shape::Bitmap => Value::Int(8),
        Shape::PanTilt => Value::PanTilt {
            pan: -36_000,
            tilt: 18_000,
        },
        Shape::PanTiltRelative => Value::PanTiltSpeed {
            pan: -1,
            pan_speed: 3,
            tilt: 1,
            tilt_speed: 4,
        },
        Shape::ZoomRelative => Value::ZoomSpeed {
            direction: -1,
            digital: true,
            speed: 2,
        },
        Shape::DirectionSpeed => Value::DirectionSpeed {
            direction: 1,
            speed: 9,
        },
        Shape::WhiteBalanceComponent => Value::WhiteBalance {
            blue: 1024,
            red: 1251,
        },
    }
}

#[test]
fn every_control_round_trips_through_its_wire_form() {
    for c in Control::ALL {
        let shape = c.spec().shape;
        let v = sample(shape);
        let bytes = v.encode(shape).unwrap();
        assert_eq!(bytes.len(), usize::from(shape.size()), "{c:?}");
        assert_eq!(Value::decode(shape, &bytes).unwrap(), v, "{c:?}");
    }
}

#[test]
fn decoding_short_input_is_an_error_not_a_panic() {
    for c in Control::ALL {
        let shape = c.spec().shape;
        let short = vec![0; usize::from(shape.size()) - 1];
        assert!(Value::decode(shape, &short).is_err(), "{c:?}");
    }
}

#[test]
fn switches_and_mode_bitmaps_are_checked() {
    let bool_shape = Control::FocusAuto.spec().shape;
    assert!(Value::Int(2).encode(bool_shape).is_err());
    let ae = Control::AeMode.spec().shape;
    assert!(Value::Int(0b0011).encode(ae).is_err(), "two modes at once");
    assert!(Value::Int(0).encode(ae).is_err(), "no mode");
    assert!(Value::Int(0b0010).encode(ae).is_ok());
}

#[test]
fn non_finite_or_huge_degrees_are_refused() {
    assert!(Value::pan_tilt_from_degrees(f64::NAN, 0.0).is_err());
    assert!(Value::pan_tilt_from_degrees(0.0, f64::INFINITY).is_err());
    assert!(Value::pan_tilt_from_degrees(1e9, 0.0).is_err());
    assert_eq!(
        Value::pan_tilt_from_degrees(-20.0, 5.5).unwrap(),
        Value::PanTilt {
            pan: -72_000,
            tilt: 19_800
        }
    );
}

#[test]
fn eight_bit_signed_scalars_sign_extend() {
    let shape = Control::ExposureTimeRelative.spec().shape;
    assert_eq!(Value::decode(shape, &[0xff]).unwrap(), Value::Int(-1));
}

use camctl_uvc::Control;

#[test]
fn every_control_has_a_unique_kebab_case_key() {
    let mut seen = std::collections::HashSet::new();
    for c in Control::ALL {
        let key = c.key();
        assert!(
            key.chars().all(|ch| ch.is_ascii_lowercase() || ch == '-'),
            "{key} is not kebab-case"
        );
        assert!(seen.insert(key), "duplicate key {key}");
    }
}

#[test]
fn keys_round_trip() {
    for c in Control::ALL {
        assert_eq!(Control::from_key(c.key()), Some(c));
    }
}

#[test]
fn common_keys_are_short() {
    assert_eq!(Control::ZoomAbsolute.key(), "zoom");
    assert_eq!(Control::PanTiltAbsolute.key(), "pan-tilt");
    assert_eq!(
        Control::WhiteBalanceTemperatureAuto.key(),
        "white-balance-auto"
    );
    assert_eq!(Control::from_key("nope"), None);
}

//! Remembering where a camera pointed, across launches.

use camctl_core::pose::Pose;

const POSE: Pose = Pose {
    pan: -12.5,
    tilt: 4.0,
    zoom: 30.0,
};

#[test]
fn a_pose_survives_its_text_form() {
    assert_eq!(Pose::decode(&POSE.encode()), Some(POSE));
}

#[test]
fn the_text_form_is_readable_in_defaults() {
    assert_eq!(POSE.encode(), "pan -12.5 tilt 4 zoom 30");
}

#[test]
fn text_that_is_not_a_pose_decodes_to_nothing() {
    for text in [
        "",
        "pan 1 tilt 2",
        "pan 1 tilt 2 zoom 3 extra 4",
        "tilt 1 pan 2 zoom 3",
        "pan one tilt 2 zoom 3",
        "pan NaN tilt 2 zoom 3",
        "pan 1 tilt inf zoom 3",
    ] {
        assert_eq!(Pose::decode(text), None, "{text:?}");
    }
}

#[test]
fn a_pose_that_is_not_finite_is_not_worth_keeping() {
    assert!(POSE.is_finite());
    assert!(
        !Pose {
            zoom: f64::NAN,
            ..POSE
        }
        .is_finite()
    );
}

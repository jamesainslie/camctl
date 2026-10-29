//! Settings kept across launches, in the app's user defaults
//! (`defaults read us.ainslies.camctl`).

use camctl_core::pose::Pose;
use objc2::rc::Retained;
use objc2_foundation::{NSString, NSUserDefaults, ns_string};

/// The AVFoundation unique ID of the camera the user chose.
fn camera_key() -> &'static NSString {
    ns_string!("camera")
}

/// The camera the user chose last, if they chose one.
pub fn camera() -> Option<String> {
    NSUserDefaults::standardUserDefaults()
        .stringForKey(camera_key())
        .map(|id| id.to_string())
}

/// Remember the camera the user chose.
pub fn set_camera(unique_id: &str) {
    let id = NSString::from_str(unique_id);
    // SAFETY: an NSString is a valid property-list value for user defaults.
    unsafe { NSUserDefaults::standardUserDefaults().setObject_forKey(Some(&id), camera_key()) };
}

/// The key for where the camera with `unique_id` last pointed.
fn pose_key(unique_id: &str) -> Retained<NSString> {
    NSString::from_str(&format!("pose.{unique_id}"))
}

/// Where the camera with `unique_id` pointed when camctl last knew, if it
/// has been seen before.
pub fn pose(unique_id: &str) -> Option<Pose> {
    let text = NSUserDefaults::standardUserDefaults().stringForKey(&pose_key(unique_id))?;
    Pose::decode(&text.to_string())
}

/// Remember where the camera with `unique_id` points.
pub fn set_pose(unique_id: &str, pose: Pose) {
    if !pose.is_finite() {
        return;
    }
    let text = NSString::from_str(&pose.encode());
    // SAFETY: an NSString is a valid property-list value for user defaults.
    unsafe {
        NSUserDefaults::standardUserDefaults().setObject_forKey(Some(&text), &pose_key(unique_id));
    };
}

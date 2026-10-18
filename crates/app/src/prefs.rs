//! Settings kept across launches, in the app's user defaults
//! (`defaults read us.ainslies.camctl`).

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

//! What happens around camctl that it must follow: cameras connected and
//! disconnected (a camera re-enumerating after the Mac sleeps posts both),
//! the Mac waking, and a second launch asking this one to show itself.
//!
//! Without these, a camera unplugged and plugged back, or one that
//! re-enumerated during sleep, stayed lost until camctl was restarted: its
//! capture and USB handles were dead and nothing reopened them.

use futures::channel::mpsc::UnboundedSender;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, Sel};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationCenter,
    NSNotificationSuspensionBehavior, NSString, ns_string,
};

/// Posted by a second launch, observed by the running camctl.
fn show_request() -> &'static NSString {
    ns_string!("us.ainslies.camctl.show")
}

/// Something camctl should react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemEvent {
    /// A camera appeared, by unique ID when AVFoundation says which.
    CameraConnected(Option<String>),
    CameraDisconnected(Option<String>),
    /// The Mac woke from sleep.
    Woke,
    /// Another launch of camctl asked this one to show its preview.
    ShowRequested,
}

#[derive(Debug)]
struct Ivars {
    tx: UnboundedSender<SystemEvent>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the only ivar is a
    // channel sender, which is Send + Sync, so notifications may arrive on
    // any thread.
    #[unsafe(super(NSObject))]
    #[name = "CamctlSystemObserver"]
    #[ivars = Ivars]
    #[derive(Debug)]
    struct SystemObserver;

    // SAFETY: NSObjectProtocol has no requirements beyond being an NSObject.
    unsafe impl NSObjectProtocol for SystemObserver {}

    impl SystemObserver {
        // SAFETY (all four): notification observer methods,
        // `void (NSNotification *)`, matching the selectors registered in
        // `watch`.
        #[unsafe(method(cameraConnected:))]
        fn camera_connected(&self, note: &NSNotification) {
            self.send(SystemEvent::CameraConnected(camera_of(note)));
        }

        #[unsafe(method(cameraDisconnected:))]
        fn camera_disconnected(&self, note: &NSNotification) {
            self.send(SystemEvent::CameraDisconnected(camera_of(note)));
        }

        #[unsafe(method(woke:))]
        fn woke(&self, _note: &NSNotification) {
            self.send(SystemEvent::Woke);
        }

        #[unsafe(method(showRequested:))]
        fn show_requested(&self, _note: &NSNotification) {
            self.send(SystemEvent::ShowRequested);
        }
    }
);

fn camera_of(note: &NSNotification) -> Option<String> {
    camctl_capture::unique_id_of(&*note.object()?)
}

impl SystemObserver {
    fn new(tx: UnboundedSender<SystemEvent>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { tx });
        // SAFETY: NSObject's designated initializer on a fresh allocation.
        unsafe { msg_send![super(this), init] }
    }

    fn send(&self, event: SystemEvent) {
        // A closed channel means the app is quitting.
        let _ = self.ivars().tx.unbounded_send(event);
    }
}

/// Observes until dropped.
#[derive(Debug)]
pub struct SystemWatch {
    local: Retained<NSNotificationCenter>,
    workspace: Retained<NSNotificationCenter>,
    distributed: Retained<NSDistributedNotificationCenter>,
    observer: Retained<SystemObserver>,
}

impl Drop for SystemWatch {
    fn drop(&mut self) {
        // SAFETY: the observer was registered with these centers in `watch`.
        unsafe {
            self.local.removeObserver(&self.observer);
            self.workspace.removeObserver(&self.observer);
            self.distributed.removeObserver(&self.observer);
        }
    }
}

/// Start watching; events arrive on `tx`.
pub fn watch(tx: UnboundedSender<SystemEvent>) -> SystemWatch {
    let observer = SystemObserver::new(tx);
    let local = NSNotificationCenter::defaultCenter();
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    let distributed = NSDistributedNotificationCenter::defaultCenter();
    let (connected, disconnected) = camctl_capture::device_notifications();
    // SAFETY: AppKit's constant, always present on macOS.
    let did_wake = unsafe { NSWorkspaceDidWakeNotification };
    let local_names: [(&NSString, Sel); 2] = [
        (connected, sel!(cameraConnected:)),
        (disconnected, sel!(cameraDisconnected:)),
    ];
    // SAFETY: each selector (in every registration below) is a method of
    // SystemObserver taking one NSNotification; the observer stays
    // registered until SystemWatch drops, which removes it.
    unsafe {
        for (name, selector) in local_names {
            local.addObserver_selector_name_object(&observer, selector, Some(name), None);
        }
        workspace.addObserver_selector_name_object(&observer, sel!(woke:), Some(did_wake), None);
        // Delivered at once: a menu bar app is almost never the active app,
        // and distributed notifications to inactive apps wait otherwise.
        distributed.addObserver_selector_name_object_suspensionBehavior(
            &observer,
            sel!(showRequested:),
            Some(show_request()),
            None,
            NSNotificationSuspensionBehavior::DeliverImmediately,
        );
    }
    SystemWatch {
        local,
        workspace,
        distributed,
        observer,
    }
}

/// Ask the camctl already running to show its preview.
pub fn ask_running_instance_to_show() {
    // SAFETY: a nil object and user info are allowed.
    unsafe {
        NSDistributedNotificationCenter::defaultCenter()
            .postNotificationName_object_userInfo_deliverImmediately(
                show_request(),
                None,
                None,
                true,
            );
    }
}

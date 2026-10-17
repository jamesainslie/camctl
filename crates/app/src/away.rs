//! The screen saver and the lock screen: the user is away, so the camera
//! should be off. macOS does not count the screen saver as covering other
//! windows, so window visibility alone would leave the camera streaming
//! (and its light on) under it.
//!
//! Distributed notifications are held back while an app is inactive unless
//! the observer asks for immediate delivery, and a menu bar app is almost
//! always inactive. So this registers a small observer object with
//! `DeliverImmediately` rather than using the block API, which coalesces.

use futures::channel::mpsc::UnboundedSender;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, Sel};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationSuspensionBehavior, NSString,
};

/// A change in why the user may be away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwayEvent {
    ScreenSaver(bool),
    Locked(bool),
}

#[derive(Debug)]
struct Ivars {
    tx: UnboundedSender<AwayEvent>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the only ivar is a
    // channel sender, which is Send + Sync, so notifications may arrive on
    // any thread.
    #[unsafe(super(NSObject))]
    #[name = "CamctlAwayObserver"]
    #[ivars = Ivars]
    #[derive(Debug)]
    struct AwayObserver;

    // SAFETY: NSObjectProtocol has no requirements beyond being an NSObject.
    unsafe impl NSObjectProtocol for AwayObserver {}

    impl AwayObserver {
        // SAFETY (all four): notification observer methods,
        // `void (NSNotification *)`, matching the selectors registered in
        // `watch`.
        #[unsafe(method(screenSaverStarted:))]
        fn screen_saver_started(&self, _note: &NSNotification) {
            self.send(AwayEvent::ScreenSaver(true));
        }

        #[unsafe(method(screenSaverStopped:))]
        fn screen_saver_stopped(&self, _note: &NSNotification) {
            self.send(AwayEvent::ScreenSaver(false));
        }

        #[unsafe(method(screenLocked:))]
        fn screen_locked(&self, _note: &NSNotification) {
            self.send(AwayEvent::Locked(true));
        }

        #[unsafe(method(screenUnlocked:))]
        fn screen_unlocked(&self, _note: &NSNotification) {
            self.send(AwayEvent::Locked(false));
        }
    }
);

impl AwayObserver {
    fn new(tx: UnboundedSender<AwayEvent>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { tx });
        // SAFETY: NSObject's designated initializer on a fresh allocation.
        unsafe { msg_send![super(this), init] }
    }

    fn send(&self, event: AwayEvent) {
        // A closed channel means the app is quitting.
        let _ = self.ivars().tx.unbounded_send(event);
    }
}

/// Observes the system notifications until dropped.
#[derive(Debug)]
pub struct AwayWatch {
    center: Retained<NSDistributedNotificationCenter>,
    observer: Retained<AwayObserver>,
}

impl Drop for AwayWatch {
    fn drop(&mut self) {
        // SAFETY: the observer was registered with this center in `watch`.
        unsafe { self.center.removeObserver(&self.observer) };
    }
}

/// Start watching; events arrive on `tx` as they happen, whether or not
/// camctl is the active app.
pub fn watch(tx: UnboundedSender<AwayEvent>) -> AwayWatch {
    let center = NSDistributedNotificationCenter::defaultCenter();
    let observer = AwayObserver::new(tx);
    let names: [(&str, Sel); 4] = [
        ("com.apple.screensaver.didstart", sel!(screenSaverStarted:)),
        ("com.apple.screensaver.didstop", sel!(screenSaverStopped:)),
        ("com.apple.screenIsLocked", sel!(screenLocked:)),
        ("com.apple.screenIsUnlocked", sel!(screenUnlocked:)),
    ];
    for (name, selector) in names {
        // SAFETY: each selector is a method of AwayObserver taking one
        // NSNotification; the observer stays registered until AwayWatch
        // drops, which removes it.
        unsafe {
            center.addObserver_selector_name_object_suspensionBehavior(
                &observer,
                selector,
                Some(&NSString::from_str(name)),
                None,
                NSNotificationSuspensionBehavior::DeliverImmediately,
            );
        }
    }
    AwayWatch { center, observer }
}

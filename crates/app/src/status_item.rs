//! The menu bar icon. GPUI has no status item support, so this is a thin
//! AppKit layer: left click toggles the preview window, right click (or
//! control-click) opens a native menu, which also chooses the camera and
//! puts it to sleep or wakes it. Clicks become [`StatusEvent`]s on a
//! channel that the GPUI side drains on its own executor.
//!
//! The icon is drawn in code as a template image (a gimbal webcam), so it
//! is crisp at any scale and follows the menu bar's light or dark look. The
//! lens fills in while the camera is streaming.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;

use block2::RcBlock;
use futures::channel::mpsc::UnboundedSender;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSBezierPath, NSColor, NSControlStateValueOn, NSEventMask, NSEventModifierFlags,
    NSEventType, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength, NSWindingRule,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};

/// What the user asked for from the menu bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusEvent {
    TogglePreview,
    ToggleSleep,
    /// Preview and control the camera with this AVFoundation unique ID.
    ChooseCamera(String),
    /// Close the camera and open it afresh, for when it has stopped
    /// answering.
    Reconnect,
    Quit,
}

#[derive(Debug)]
struct Ivars {
    tx: UnboundedSender<StatusEvent>,
    item: OnceCell<Retained<NSStatusItem>>,
    menu: OnceCell<Retained<NSMenu>>,
    /// The Camera submenu, filled afresh each time the menu opens so it
    /// lists the cameras connected right then.
    cameras: OnceCell<Retained<NSMenu>>,
    /// Unique IDs of the cameras in the submenu, indexed by item tag.
    camera_ids: RefCell<Vec<String>>,
    /// The camera the preview shows, checked in the submenu.
    current: RefCell<Option<String>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. The class is
    // MainThreadOnly, so its ivars (which are not Send) are only touched on
    // the main thread, where AppKit delivers the actions below.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CamctlStatusTarget"]
    #[ivars = Ivars]
    #[derive(Debug)]
    struct StatusTarget;

    // SAFETY: NSObjectProtocol has no requirements beyond being an NSObject.
    unsafe impl NSObjectProtocol for StatusTarget {}

    impl StatusTarget {
        // SAFETY: an action method: `void (id sender)`, matching the
        // selector the status bar button and menu items send.
        #[unsafe(method(statusClicked:))]
        fn status_clicked(&self, _sender: Option<&AnyObject>) {
            if self.is_menu_click() {
                self.show_menu();
            } else {
                self.send(StatusEvent::TogglePreview);
            }
        }

        // SAFETY: as above.
        #[unsafe(method(togglePreview:))]
        fn toggle_preview(&self, _sender: Option<&AnyObject>) {
            self.send(StatusEvent::TogglePreview);
        }

        // SAFETY: as above.
        #[unsafe(method(toggleSleep:))]
        fn toggle_sleep(&self, _sender: Option<&AnyObject>) {
            self.send(StatusEvent::ToggleSleep);
        }

        // SAFETY: as above.
        #[unsafe(method(chooseCamera:))]
        fn choose_camera(&self, sender: Option<&AnyObject>) {
            let Some(tag) = sender
                .and_then(AnyObject::downcast_ref::<NSMenuItem>)
                .and_then(|item| usize::try_from(item.tag()).ok())
            else {
                return;
            };
            let id = self.ivars().camera_ids.borrow().get(tag).cloned();
            if let Some(id) = id {
                self.send(StatusEvent::ChooseCamera(id));
            }
        }

        // SAFETY: as above.
        #[unsafe(method(reconnect:))]
        fn reconnect(&self, _sender: Option<&AnyObject>) {
            self.send(StatusEvent::Reconnect);
        }

        // SAFETY: as above.
        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            self.send(StatusEvent::Quit);
        }
    }
);

impl StatusTarget {
    fn new(mtm: MainThreadMarker, tx: UnboundedSender<StatusEvent>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            tx,
            item: OnceCell::new(),
            menu: OnceCell::new(),
            cameras: OnceCell::new(),
            camera_ids: RefCell::new(Vec::new()),
            current: RefCell::new(None),
        });
        // SAFETY: NSObject's designated initializer, called once on a fresh
        // allocation.
        unsafe { msg_send![super(this), init] }
    }

    fn send(&self, event: StatusEvent) {
        // The receiver lives as long as the app; a send can only fail while
        // the app is shutting down, when there is nothing left to do.
        let _ = self.ivars().tx.unbounded_send(event);
    }

    /// Right click, or control-click, opens the menu.
    fn is_menu_click(&self) -> bool {
        let app = NSApplication::sharedApplication(self.mtm());
        app.currentEvent().is_some_and(|event| {
            event.r#type() == NSEventType::RightMouseUp
                || event
                    .modifierFlags()
                    .contains(NSEventModifierFlags::Control)
        })
    }

    fn show_menu(&self) {
        let (Some(item), Some(menu)) = (self.ivars().item.get(), self.ivars().menu.get()) else {
            return;
        };
        let Some(button) = item.button(self.mtm()) else {
            return;
        };
        self.fill_cameras();
        // Attach the menu only for this click, so a plain left click keeps
        // toggling the window instead of opening the menu.
        item.setMenu(Some(menu));
        // SAFETY: a nil sender is allowed; the button is alive (we hold it).
        unsafe { button.performClick(None) };
        item.setMenu(None);
    }

    /// List the connected cameras in the Camera submenu, the current one
    /// checked.
    fn fill_cameras(&self) {
        let Some(menu) = self.ivars().cameras.get() else {
            return;
        };
        let mtm = self.mtm();
        menu.removeAllItems();
        let devices = camctl_capture::devices();
        let names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
        let current = self.ivars().current.borrow().clone();
        for (tag, (device, title)) in devices.iter().zip(camera_titles(&names)).enumerate() {
            let item = menu_item(
                mtm,
                &NSString::from_str(&title),
                sel!(chooseCamera:),
                ns_string!(""),
                self,
            );
            item.setTag(isize::try_from(tag).unwrap_or(isize::MAX));
            if current.as_deref() == Some(device.unique_id.as_str()) {
                item.setState(NSControlStateValueOn);
            }
            menu.addItem(&item);
        }
        if devices.is_empty() {
            // No action, so the menu shows it disabled.
            menu.addItem(&info_item(mtm, ns_string!("No Cameras Connected")));
        }
        *self.ivars().camera_ids.borrow_mut() = devices.into_iter().map(|d| d.unique_id).collect();
    }
}

/// Menu titles for cameras with these names, in order. Names shared by
/// several cameras (two of the same model, say) are numbered so each can
/// be told apart.
fn camera_titles(names: &[&str]) -> Vec<String> {
    let mut seen = HashMap::<&str, usize>::new();
    names
        .iter()
        .map(|&name| {
            if names.iter().filter(|&&n| n == name).count() == 1 {
                return name.to_owned();
            }
            let n = seen.entry(name).or_default();
            *n += 1;
            format!("{name} ({n})")
        })
        .collect()
}

/// Size of the icon in points (the menu bar is 22 pt tall).
const ICON: f64 = 18.0;

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

/// The camctl glyph: a camera head on a gimbal stem and base, with the
/// lens cut out. `live` fills the lens centre. Coordinates are in points,
/// origin bottom-left.
fn draw_glyph(live: bool) {
    NSColor::blackColor().set();

    // Head with the lens cut out (even-odd fill leaves the ring empty).
    let head = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        rect(2.0, 6.0, 14.0, 10.0),
        3.4,
        3.4,
    );
    head.appendBezierPathWithOvalInRect(rect(5.2, 7.4, 7.6, 7.6));
    head.setWindingRule(NSWindingRule::EvenOdd);
    head.fill();

    // Lens: a small glint when idle, a filled centre while streaming.
    let lens = if live {
        rect(6.9, 9.1, 4.2, 4.2)
    } else {
        rect(9.6, 11.6, 1.6, 1.6)
    };
    NSBezierPath::bezierPathWithOvalInRect(lens).fill();

    // Gimbal stem and base.
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect(8.1, 3.4, 1.8, 3.0), 0.6, 0.6)
        .fill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect(4.5, 1.4, 9.0, 2.4), 1.2, 1.2)
        .fill();
}

/// A template image of the glyph, drawn at whatever scale it is shown.
fn icon(live: bool) -> Retained<NSImage> {
    let handler = RcBlock::new(move |_: NSRect| -> Bool {
        draw_glyph(live);
        Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(ICON, ICON), false, &handler);
    image.setTemplate(true);
    image.setAccessibilityDescription(Some(if live {
        ns_string!("camctl, camera on")
    } else {
        ns_string!("camctl")
    }));
    image
}

/// Keeps the status item and its target alive; dropping it removes the icon.
#[derive(Debug)]
pub struct StatusItem {
    mtm: MainThreadMarker,
    item: Retained<NSStatusItem>,
    idle: Retained<NSImage>,
    live: Retained<NSImage>,
    sleep: Retained<NSMenuItem>,
    target: Retained<StatusTarget>,
}

impl StatusItem {
    /// Show whether the camera is streaming.
    pub fn set_live(&self, live: bool) {
        if let Some(button) = self.item.button(self.mtm) {
            button.setImage(Some(if live { &self.live } else { &self.idle }));
        }
    }

    /// Offer to wake the camera while it sleeps, and to sleep otherwise.
    pub fn set_asleep(&self, asleep: bool) {
        self.sleep.setTitle(sleep_title(asleep));
    }

    /// The camera the preview shows, by unique ID, to check in the menu.
    pub fn set_camera(&self, unique_id: Option<&str>) {
        *self.target.ivars().current.borrow_mut() = unique_id.map(str::to_owned);
    }
}

fn sleep_title(asleep: bool) -> &'static NSString {
    if asleep {
        ns_string!("Wake Camera")
    } else {
        ns_string!("Sleep Camera")
    }
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &NSString,
    action: objc2::runtime::Sel,
    key: &NSString,
    target: &StatusTarget,
) -> Retained<NSMenuItem> {
    // SAFETY: the selectors are methods defined on StatusTarget above, and
    // the target outlives the menu (both are owned by StatusItem).
    unsafe {
        let item = NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            title,
            Some(action),
            key,
        );
        item.setTarget(Some(target));
        item
    }
}

/// A menu item with no action: shown disabled, unless it opens a submenu.
fn info_item(mtm: MainThreadMarker, title: &NSString) -> Retained<NSMenuItem> {
    // SAFETY: a plain item with no action or target.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            title,
            None,
            ns_string!(""),
        )
    }
}

/// Put the camctl icon in the menu bar. Main thread only.
pub fn install(mtm: MainThreadMarker, tx: UnboundedSender<StatusEvent>) -> StatusItem {
    let target = StatusTarget::new(mtm, tx);
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    let (idle, live) = (icon(false), icon(true));

    if let Some(button) = item.button(mtm) {
        button.setImage(Some(&idle));
        // SAFETY: statusClicked: is defined on StatusTarget, which StatusItem
        // keeps alive for as long as the button exists.
        unsafe {
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(statusClicked:)));
        }
        button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
        button.setToolTip(Some(ns_string!(
            "camctl: click to show or hide the preview"
        )));
    }

    let menu = NSMenu::new(mtm);
    menu.addItem(&menu_item(
        mtm,
        ns_string!("Show or Hide Preview"),
        sel!(togglePreview:),
        ns_string!(""),
        &target,
    ));
    let cameras = NSMenu::new(mtm);
    let camera = info_item(mtm, ns_string!("Camera"));
    camera.setSubmenu(Some(&cameras));
    menu.addItem(&camera);
    let sleep = menu_item(
        mtm,
        sleep_title(false),
        sel!(toggleSleep:),
        ns_string!(""),
        &target,
    );
    menu.addItem(&sleep);
    menu.addItem(&menu_item(
        mtm,
        ns_string!("Reconnect Camera"),
        sel!(reconnect:),
        ns_string!(""),
        &target,
    ));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&menu_item(
        mtm,
        ns_string!("Quit camctl"),
        sel!(quit:),
        ns_string!("q"),
        &target,
    ));

    let _ = target.ivars().item.set(item.clone());
    let _ = target.ivars().menu.set(menu);
    let _ = target.ivars().cameras.set(cameras);
    StatusItem {
        mtm,
        item,
        idle,
        live,
        sleep,
        target,
    }
}

#[cfg(test)]
mod tests {
    use super::camera_titles;

    #[test]
    fn distinct_names_stay_as_they_are() {
        assert_eq!(
            camera_titles(&["OBSBOT Tiny 2 Lite", "FaceTime HD Camera"]),
            ["OBSBOT Tiny 2 Lite", "FaceTime HD Camera"]
        );
    }

    #[test]
    fn repeated_names_are_numbered_in_order() {
        assert_eq!(
            camera_titles(&[
                "OBSBOT Tiny 2 Lite",
                "FaceTime HD Camera",
                "OBSBOT Tiny 2 Lite"
            ]),
            [
                "OBSBOT Tiny 2 Lite (1)",
                "FaceTime HD Camera",
                "OBSBOT Tiny 2 Lite (2)"
            ]
        );
    }
}

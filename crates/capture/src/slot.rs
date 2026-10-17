//! A one-item mailbox between the capture queue and the UI.
//!
//! The producer replaces whatever is there; the consumer takes it. The
//! producer wakes the consumer only when it fills an *empty* slot, so a busy
//! consumer is woken once per batch of frames, not once per frame.
//!
//! The rule that keeps this working: **every wake must be answered with a
//! `take`**, even by a consumer that is going to discard the item. A slot
//! left full means no further wakes.

use std::sync::{Mutex, PoisonError};

/// Holds at most one item, the newest.
#[derive(Debug)]
pub struct LatestSlot<T> {
    item: Mutex<Option<T>>,
}

impl<T> Default for LatestSlot<T> {
    fn default() -> Self {
        LatestSlot {
            item: Mutex::new(None),
        }
    }
}

impl<T> LatestSlot<T> {
    /// Store `item`, dropping any older one. Returns `true` when the slot
    /// was empty, which is when the consumer must be woken.
    pub fn put(&self, item: T) -> bool {
        self.item
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(item)
            .is_none()
    }

    /// Take the newest item, leaving the slot empty so the next `put`
    /// wakes the consumer again.
    pub fn take(&self) -> Option<T> {
        self.item
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filling_an_empty_slot_asks_for_a_wake() {
        let slot = LatestSlot::default();
        assert!(slot.put(1));
    }

    #[test]
    fn filling_a_full_slot_does_not_and_keeps_the_newest() {
        let slot = LatestSlot::default();
        assert!(slot.put(1));
        assert!(!slot.put(2));
        assert!(!slot.put(3));
        assert_eq!(slot.take(), Some(3));
    }

    #[test]
    fn taking_re_arms_the_wake() {
        let slot = LatestSlot::default();
        slot.put(1);
        assert_eq!(slot.take(), Some(1));
        assert!(slot.put(2), "the next frame wakes the consumer again");
        assert_eq!(slot.take(), Some(2));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn a_consumer_that_skips_a_take_stops_being_woken() {
        // The bug this type's rule exists for: a paused consumer that
        // ignored its wake without taking left the slot full, and no frame
        // after a resume ever woke it.
        let slot = LatestSlot::default();
        assert!(slot.put("before pause"));
        // ... the consumer is paused and does not take ...
        assert!(!slot.put("after resume"), "no wake: the slot is still full");
        // Taking, even only to discard, is what restores the wakes.
        let _ = slot.take();
        assert!(slot.put("next"));
    }
}

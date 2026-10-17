//! camctl's app logic, independent of UI and I/O: coalesced control
//! writes and reads, smooth motion, and per-camera quirks.

pub mod motion;
mod queue;
pub mod quirks;

pub use queue::{ControlQueue, Pending, QueueEvent, ReadOutcome, WriteOutcome};

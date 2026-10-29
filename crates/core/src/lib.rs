//! camctl's app logic, independent of UI and I/O: coalesced control
//! writes and reads, control-link health, smooth motion, remembered poses, and
//! per-camera quirks.

pub mod health;
pub mod motion;
pub mod pose;
mod queue;
pub mod quirks;

pub use queue::{ControlQueue, Pending, QueueEvent, ReadOutcome, WriteOutcome};

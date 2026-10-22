//! camctl's app logic, independent of UI and I/O: coalesced control
//! writes and reads, control-link health, smooth motion, and per-camera
//! quirks.

pub mod health;
pub mod motion;
mod queue;
pub mod quirks;

pub use queue::{ControlQueue, Pending, QueueEvent, ReadOutcome, WriteOutcome};

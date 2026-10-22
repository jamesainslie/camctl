//! Whether the USB control link to a camera still works.
//!
//! A camera that re-enumerates (the Mac woke from sleep, a hub reset, the
//! camera's own deep sleep) leaves an opened handle dead: every transfer on
//! it fails from then on, and nothing else says so. The control results
//! say it, so each one is fed in here, and a dead link is reopened.

use std::time::Duration;

use camctl_uvc::{CameraError, TransportError};

/// Failures in a row, with no answer from the camera between them, that
/// mean the link is gone. A busy camera may time out once or twice.
const FAILURES_FOR_RECONNECT: u32 = 3;

/// What to do about the link after one result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Fine,
    /// The handle is dead: drop it and open the camera again.
    Reconnect,
}

/// Counts failures in a row on one control link.
#[derive(Debug, Default)]
pub struct ControlHealth {
    failures: u32,
}

impl ControlHealth {
    /// Note one control result and say whether the link needs reopening.
    pub fn record<T>(&mut self, result: &Result<T, CameraError>) -> Verdict {
        let source = match result {
            Ok(_) => {
                self.failures = 0;
                return Verdict::Fine;
            }
            Err(CameraError::Request { source, .. }) => source,
            // Refused before any I/O (an unsupported control, a bad
            // value): nothing about the link.
            Err(_) => return Verdict::Fine,
        };
        match source {
            // The camera answered, just not with a yes.
            TransportError::Stall => {
                self.failures = 0;
                Verdict::Fine
            }
            TransportError::Disconnected => self.reconnect(),
            _ => {
                self.failures += 1;
                if self.failures >= FAILURES_FOR_RECONNECT {
                    self.reconnect()
                } else {
                    Verdict::Fine
                }
            }
        }
    }

    fn reconnect(&mut self) -> Verdict {
        self.failures = 0;
        Verdict::Reconnect
    }
}

/// How long to wait before reopening attempt `attempt` (from 0): quick at
/// first, since a re-enumerating camera is back within a second or two,
/// then every 5 s.
#[must_use]
pub fn reconnect_delay(attempt: u32) -> Duration {
    const FIRST_MS: u64 = 500;
    const MAX: Duration = Duration::from_secs(5);
    let factor = 1_u64.checked_shl(attempt.min(16)).unwrap_or(u64::MAX);
    Duration::from_millis(FIRST_MS.saturating_mul(factor)).min(MAX)
}

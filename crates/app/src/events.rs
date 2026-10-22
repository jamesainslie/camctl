//! A small persistent log of camctl's state changes (camera opened,
//! paused and why, resumed, restarted), so a problem seen in normal use can
//! be diagnosed afterwards. It records transitions only, never per-frame
//! events, so it stays tiny.
//!
//! Lines go to `~/Library/Logs/camctl/camctl.log` (moved to `camctl.log.1`
//! when it passes 1 MB, so the history before a problem survives) and,
//! with `CAMCTL_TRACE=1`, to stderr as well.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;

const MAX_BYTES: u64 = 1_000_000;

fn path() -> Option<&'static PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = PathBuf::from(std::env::var_os("HOME")?).join("Library/Logs/camctl");
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir.join("camctl.log"))
    })
    .as_ref()
}

/// Local wall-clock time as `YYYY-MM-DD HH:MM:SS.mmm`.
fn now() -> String {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = libc::time_t::try_from(since.as_secs()).unwrap_or_default();
    // SAFETY: an all-zero `tm` is valid (integers and a nullable pointer);
    // localtime_r writes only into it and reads `secs`, which outlives the
    // call. It reads TZ from the environment; camctl never changes its
    // environment, so nothing races with that read.
    let tm = unsafe {
        let mut tm = std::mem::zeroed::<libc::tm>();
        if libc::localtime_r(&raw const secs, &raw mut tm).is_null() {
            return format!("{}.{:03}", since.as_secs(), since.subsec_millis());
        }
        tm
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        since.subsec_millis()
    )
}

/// An error with its causes, `outer: inner: innermost`, so the log says
/// why something failed and not just what (a stall, a timeout and a dead
/// handle look the same otherwise).
pub fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Record one state change.
pub fn record(message: &str) {
    let line = format!("{} {message}\n", now());
    if std::env::var_os("CAMCTL_TRACE").is_some() {
        eprint!("camctl: {line}");
    }
    let Some(path) = path() else {
        return;
    };
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        // Best effort: if the rename fails the log just keeps growing.
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        // Logging is best effort; a full disk must not stop the camera.
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use camctl_uvc::{CameraError, Control, RequestCode, TransportError};

    use super::describe;

    #[test]
    fn the_log_shows_why_a_request_failed() {
        let e = CameraError::Request {
            control: Control::ZoomAbsolute,
            code: RequestCode::SetCur,
            source: TransportError::Disconnected,
        };
        assert_eq!(describe(&e), "Zoom: SET_CUR failed: device disconnected");
    }

    #[test]
    fn an_error_without_a_cause_is_just_itself() {
        let e = CameraError::Unsupported(Control::ZoomAbsolute);
        assert_eq!(describe(&e), e.to_string());
    }
}

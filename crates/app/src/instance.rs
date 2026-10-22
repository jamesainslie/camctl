//! One camctl at a time. Two copies (an installed one and a fresh build,
//! say) would show two menu bar icons, both drive the camera, and fight
//! over it. macOS only stops a second launch of the same bundle path, so
//! this holds an advisory lock for the life of the process; the kernel
//! drops it when the process exits, crashed or not.
//!
//! A second launch asks the running one to show its preview, then quits.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd as _;
use std::path::{Path, PathBuf};

/// Held while this process is the one camctl.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

/// Where the lock lives: `~/Library/Application Support/camctl`.
pub fn default_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/camctl/instance.lock"))
}

/// Take the lock at `path`. `Ok(None)` when another process holds it.
pub fn acquire(path: &Path) -> io::Result<Option<InstanceLock>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    // SAFETY: flock takes a file descriptor and flags; the descriptor is
    // open (we own `file`) for the whole call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(InstanceLock { _file: file }));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(None)
    } else {
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::acquire;

    #[test]
    fn only_one_holder_at_a_time() {
        let dir = std::env::temp_dir().join(format!("camctl-instance-{}", std::process::id()));
        let path = dir.join("instance.lock");
        let first = acquire(&path).unwrap();
        assert!(first.is_some(), "the first launch gets the lock");
        assert!(
            acquire(&path).unwrap().is_none(),
            "a second launch does not"
        );
        drop(first);
        assert!(
            acquire(&path).unwrap().is_some(),
            "free again once released"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

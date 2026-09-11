//! Single-instance guard.
//!
//! A second Scriblet process would install a second keyboard hook and expand
//! every trigger twice. Newer Scriblet versions hold an OS-level file lock for
//! their lifetime. On Windows we also detect an already-running legacy Scriblet
//! window, because v0.5.0 and earlier did not acquire this lock and may still be
//! alive in the tray during an upgrade.

use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::path::Path;

pub const LOCK_FILE_NAME: &str = "scriblet.lock";

/// Held for the life of the process. Dropping it releases the lock.
pub struct InstanceLock {
    _file: File,
}

/// Returns true when a pre-lock Windows Scriblet instance is still alive.
///
/// Scriblet creates its main window after acquiring the instance guard, so an
/// existing top-level window with the exact application title must belong to
/// another process. Hidden tray windows still exist and are found by
/// `FindWindowW`, which is exactly the legacy-upgrade case we need to catch.
#[cfg(target_os = "windows")]
fn legacy_windows_instance_running() -> bool {
    use std::ptr::null;
    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;

    let title: Vec<u16> = "Scriblet\0".encode_utf16().collect();
    unsafe { !FindWindowW(null(), title.as_ptr()).is_null() }
}

#[cfg(not(target_os = "windows"))]
fn legacy_windows_instance_running() -> bool {
    false
}

/// Tries to become the single running instance. Returns `None` when another
/// process already owns the lock or, on Windows, when a legacy Scriblet window
/// is still alive in the tray.
pub fn acquire(data_dir: &Path) -> Result<Option<InstanceLock>> {
    if legacy_windows_instance_running() {
        return Ok(None);
    }

    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("failed to create {}", data_dir.display()))?;
    let path = data_dir.join(LOCK_FILE_NAME);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;

    match file.try_lock() {
        Ok(()) => Ok(Some(InstanceLock { _file: file })),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("failed to lock {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_until_first_is_released() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let first = acquire(dir.path())?.expect("first instance acquires the lock");
        assert!(
            acquire(dir.path())?.is_none(),
            "second instance must be refused"
        );
        drop(first);
        assert!(acquire(dir.path())?.is_some(), "lock is released on drop");
        Ok(())
    }
}

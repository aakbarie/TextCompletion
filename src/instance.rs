//! Single-instance guard.
//!
//! A second Scriblet process would install a second keyboard hook and expand
//! every trigger twice. The first process holds an OS-level lock on a file in
//! the data directory for its lifetime; later launches see the lock and exit.

use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::path::Path;

pub const LOCK_FILE_NAME: &str = "scriblet.lock";

/// Held for the life of the process. Dropping it releases the lock.
pub struct InstanceLock {
    _file: File,
}

/// Tries to become the single running instance. Returns `None` when another
/// process already holds the lock.
pub fn acquire(data_dir: &Path) -> Result<Option<InstanceLock>> {
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

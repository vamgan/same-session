use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use fs2::FileExt as _;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LockError {
    #[error("another SameSession operation holds {0}")]
    Busy(PathBuf),
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug)]
pub struct OperationLock {
    file: File,
}

impl OperationLock {
    /// Acquires an exclusive non-blocking operation lock.
    ///
    /// # Errors
    ///
    /// Returns [`LockError::Busy`] when another process owns the lock or an
    /// I/O error when the lock file cannot be created.
    pub fn acquire(path: &Path) -> Result<Self, LockError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock_exclusive().map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                LockError::Busy(path.to_path_buf())
            } else {
                LockError::Io(error)
            }
        })?;
        Ok(Self { file })
    }
}

impl Drop for OperationLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{LockError, OperationLock};

    #[test]
    fn prevents_concurrent_operation() {
        let directory = tempdir().expect("directory");
        let path = directory.path().join("operation.lock");
        let first = OperationLock::acquire(&path).expect("first");

        let error = OperationLock::acquire(&path).expect_err("must block");
        drop(first);
        OperationLock::acquire(&path).expect("after release");

        assert!(matches!(error, LockError::Busy(_)));
    }
}

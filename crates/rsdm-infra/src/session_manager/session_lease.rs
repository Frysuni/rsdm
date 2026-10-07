//! Single-session ownership that survives a restart of the user's D-Bus broker.

use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    os::{fd::AsRawFd, unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt}},
    path::{Path, PathBuf},
};

use super::SessionError;

#[derive(Debug)]
pub(super) struct SessionLease {
    _file: File,
}

impl SessionLease {
    pub fn acquire() -> Result<Self, SessionError> {
        let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
            .ok_or_else(|| SessionError::State("XDG_RUNTIME_DIR is required".into()))?;
        Self::acquire_in(&base)
    }

    pub(super) fn acquire_in(base: &Path) -> Result<Self, SessionError> {
        verify_directory(base)?;
        let directory = base.join("rsdm");
        match DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {},
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error.into()),
        }
        verify_directory(&directory)?;

        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(directory.join("session.lock"))?;
        let metadata = file.metadata()?;
        // SAFETY: geteuid has no preconditions; flock uses our owned descriptor.
        let uid = unsafe { libc::geteuid() };
        if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err(SessionError::State("unsafe session manager lease".into()));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Err(SessionError::SessionAlreadyActive);
            }
            return Err(error.into());
        }
        // Never unlink: every contender must lock the same persistent inode.
        Ok(Self { _file: file })
    }
}

fn verify_directory(path: &Path) -> Result<(), SessionError> {
    let metadata = fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if !path.is_absolute() || !metadata.is_dir() || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
    {
        return Err(SessionError::State(format!("unsafe session runtime directory {}", path.display())));
    }
    Ok(())
}

#[cfg(test)]
#[path = "session_lease_tests.rs"]
mod tests;

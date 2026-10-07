//! Validate and read one pinned lock-state inode, never reopen its pathname.

use std::{
    fs::{File, OpenOptions}, io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt}, path::Path,
};

use super::{LockControlError, LockState, STATE_VERSION};

const MAX_STATE_BYTES: u64 = 4096;

pub(super) fn read(path: &Path, uid: u32) -> Result<LockState, LockControlError> {
    let file = OpenOptions::new().read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path).map_err(|source| LockControlError::Io { path: path.to_path_buf(), source })?;
    read_file(file, path, uid)
}

fn read_file(file: File, path: &Path, uid: u32) -> Result<LockState, LockControlError> {
    let metadata = file.metadata()
        .map_err(|source| LockControlError::Io { path: path.to_path_buf(), source })?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o022 != 0
        || metadata.len() > MAX_STATE_BYTES
    {
        return Err(LockControlError::UnsafeState(path.to_path_buf()));
    }
    let mut text = String::new();
    file.take(MAX_STATE_BYTES + 1).read_to_string(&mut text)
        .map_err(|source| LockControlError::Io { path: path.to_path_buf(), source })?;
    if text.len() as u64 > MAX_STATE_BYTES {
        return Err(LockControlError::UnsafeState(path.to_path_buf()));
    }
    let state: LockState = toml::from_str(&text).map_err(LockControlError::Decode)?;
    if state.version != STATE_VERSION {
        return Err(LockControlError::UnsupportedVersion(state.version));
    }
    Ok(state)
}

#[cfg(test)]
#[path = "state_file_tests.rs"]
mod tests;

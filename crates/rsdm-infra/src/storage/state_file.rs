use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    path::Path,
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use rsdm_core::ports::StorageError;

use super::io_error;

pub(super) const MAX_STATE_BYTES: usize = 4096;

pub(super) fn read(path: &Path) -> Result<Option<String>, StorageError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    read_file(file).map(Some)
}

fn read_file(file: File) -> Result<String, StorageError> {
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() || !private_owner(&metadata) {
        return Err(StorageError::Invalid("unsafe remembered state file".into()));
    }
    if metadata.len() > MAX_STATE_BYTES as u64 {
        return Err(StorageError::Invalid("remembered state file is too large".into()));
    }

    let mut text = String::new();
    file.take((MAX_STATE_BYTES + 1) as u64).read_to_string(&mut text).map_err(io_error)?;
    if text.len() > MAX_STATE_BYTES {
        return Err(StorageError::Invalid("remembered state file is too large".into()));
    }
    Ok(text)
}

#[cfg(unix)]
fn private_owner(metadata: &std::fs::Metadata) -> bool {
    // SAFETY: geteuid has no preconditions.
    metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn private_owner(_metadata: &std::fs::Metadata) -> bool {
    true
}

#[cfg(all(test, unix))]
#[path = "state_file_tests.rs"]
mod tests;

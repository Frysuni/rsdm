//! Generation retirement survives failure to replace its mutable session record.

use super::*;
use std::io::{Seek, SeekFrom};

pub(super) fn initialize(runtime: &Runtime) -> Result<(), SessionError> {
    let path = runtime.path.with_extension("disabled");
    match OpenOptions::new().write(true).create_new(true).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(&path)
    {
        Ok(mut file) => { file.write_all(&[0])?; file.sync_all()?; }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => { marked(runtime)?; }
        Err(error) => return Err(error.into()),
    }
    Ok(sync_directory(path.parent().expect("generation marker has a parent"))?)
}

pub(super) fn mark(runtime: &Runtime) -> Result<(), SessionError> {
    let path = runtime.path.with_extension("disabled");
    let mut file = match OpenOptions::new().read(true).write(true).create_new(true).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(&path)
    {
        Ok(mut file) => { file.write_all(&[1])?; file },
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            OpenOptions::new().read(true).write(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(&path)?
        }
        Err(error) => return Err(error.into()),
    };
    file.seek(SeekFrom::Start(0))?;
    read(&mut file)?;
    // The one-byte slot is allocated before publishing the session. Updating
    // it needs no new inode or temporary file when record replacement fails.
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&[1])?;
    file.sync_all()?;
    Ok(sync_directory(path.parent().expect("generation marker has a parent"))?)
}

pub(super) fn marked(runtime: &Runtime) -> Result<bool, SessionError> {
    match open(&runtime.path.with_extension("disabled")) {
        Ok(mut file) => read(&mut file),
        Err(SessionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn open(path: &Path) -> Result<File, SessionError> {
    Ok(OpenOptions::new().read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path)?)
}

fn read(file: &mut File) -> Result<bool, SessionError> {
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0 || metadata.len() != 1
    {
        return Err(SessionError::State("unsafe disabled generation marker".into()));
    }
    let mut state = [0];
    file.read_exact(&mut state)?;
    match state[0] {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(SessionError::State("invalid disabled generation marker".into())),
    }
}

#[cfg(test)]
#[path = "runtime_disable_tests.rs"]
mod tests;

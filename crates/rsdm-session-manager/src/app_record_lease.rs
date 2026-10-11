//! Short record locks obey both local contention and shared shutdown budgets.

use std::{
    fs::{self, File, Metadata, OpenOptions},
    os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt}},
    path::Path,
    time::{Duration, Instant},
};

use super::{SessionError, deadline::Deadline};

pub(super) fn acquire(path: &Path, deadline: &Deadline) -> Result<File, SessionError> {
    let contention_deadline = Instant::now() + Duration::from_millis(250);
    loop {
        deadline.remaining(Duration::from_millis(250))?;
        if Instant::now() >= contention_deadline {
            return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "application shutdown lease changed while waiting").into());
        }
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path)?;
        lease_metadata(&file)?;
        lock_record(&file, deadline, contention_deadline)?;
        let metadata = lease_metadata(&file)?;
        // A reaper can remove an unheld inode while this descriptor waits.
        // Only the inode currently at the lease path authorizes record work.
        if matches_path(path, &metadata)? {
            deadline.remaining(Duration::MAX)?;
            return Ok(file);
        }
    }
}

fn lease_metadata(file: &File) -> Result<Metadata, SessionError> {
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(SessionError::State("unsafe application shutdown lease".into()));
    }
    Ok(metadata)
}

fn matches_path(path: &Path, metadata: &Metadata) -> Result<bool, SessionError> {
    match fs::symlink_metadata(path) {
        Ok(current) => Ok(current.is_file() && current.dev() == metadata.dev() && current.ino() == metadata.ino()
            && current.uid() == metadata.uid() && current.mode() & 0o077 == 0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn reap_closed(path: &Path) -> Result<bool, SessionError> {
    let file = match OpenOptions::new().read(true).write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let metadata = lease_metadata(&file)?;
    // SAFETY: LOCK_NB makes this probe nonblocking; a holder keeps the inode.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock { return Ok(false); }
        return Err(error.into());
    }
    if !matches_path(path, &metadata)? { return Ok(false); }
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn lock_record(file: &File, shared: &Deadline, deadline: Instant) -> Result<(), SessionError> {
    loop {
        shared.remaining(Duration::from_millis(250))?;
        // SAFETY: the descriptor is owned and LOCK_NB forbids a blocking wait.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if !matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted)
            || Instant::now() >= deadline
        {
            return Err(error.into());
        }
        let wait = shared.remaining(deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(5)))?;
        // This synchronous filesystem operation needs no async executor. Recheck
        // revisions after at most five milliseconds, before another lock try.
        std::thread::sleep(wait);
    }
}

#[cfg(test)]
#[path = "app_record_lease_tests.rs"]
mod tests;

//! Short record locks obey both local contention and shared shutdown budgets.

use std::{
    fs::{File, OpenOptions},
    os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt}},
    path::Path,
    time::{Duration, Instant},
};

use super::{SessionError, deadline::Deadline};

pub(super) fn acquire(path: &Path, deadline: &Deadline) -> Result<File, SessionError> {
    deadline.remaining(Duration::from_millis(250))?;
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path)?;
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(SessionError::State("unsafe application shutdown lease".into()));
    }
    lock_record(&file, deadline)?;
    Ok(file)
}

pub(super) fn reap_closed(path: &Path) -> Result<bool, SessionError> {
    let file = match OpenOptions::new().read(true).write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(SessionError::State("unsafe application shutdown lease".into()));
    }
    // SAFETY: LOCK_NB makes this probe nonblocking; a holder keeps the inode.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock { return Ok(false); }
        return Err(error.into());
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn lock_record(file: &File, shared: &Deadline) -> Result<(), SessionError> {
    let deadline = Instant::now() + Duration::from_millis(250);
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
        async_io::block_on(shared.bound(async {
            async_io::Timer::after(wait).await;
            Ok::<_, zbus::Error>(())
        }))?;
    }
}

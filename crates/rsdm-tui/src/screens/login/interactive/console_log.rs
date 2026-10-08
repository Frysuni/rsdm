//! Recover global console logging settings across Greeter process crashes.

use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt}},
    path::{Path, PathBuf},
};

const MAX_SNAPSHOT_BYTES: u64 = 128;

pub(super) struct ConsoleLogGuard {
    _printk: Option<PrintkGuard>,
    restore_systemd_status: bool,
}

impl ConsoleLogGuard {
    pub(super) fn quiet() -> Self {
        // Global kernel and PID 1 settings belong to the root Greeter only.
        // SAFETY: geteuid has no preconditions.
        let printk = if unsafe { libc::geteuid() } == 0 {
            match PrintkGuard::quiet_at(Path::new("/proc/sys/kernel/printk"), Path::new("/run/rsdm"), 0) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    tracing::warn!(%error, "cannot preserve console logging; leaving it unchanged");
                    None
                }
            }
        } else { None };

        // A stale snapshot also covers a possible crash after disabling PID 1
        // status output. Keep its existing enable-on-return behavior.
        if printk.as_ref().is_some_and(|guard| guard.recovered) {
            set_systemd_console_status(true);
        }
        let restore_systemd_status = printk.is_some() && set_systemd_console_status(false);
        Self { _printk: printk, restore_systemd_status }
    }
}

impl Drop for ConsoleLogGuard {
    fn drop(&mut self) {
        if self.restore_systemd_status {
            set_systemd_console_status(true);
        }
    }
}

struct PrintkGuard {
    printk: PathBuf,
    snapshot: PathBuf,
    previous: String,
    recovered: bool,
    _lease: File,
}

impl PrintkGuard {
    fn quiet_at(printk: &Path, directory: &Path, uid: u32) -> io::Result<Self> {
        let lease = acquire_lease(directory, uid)?;
        let snapshot = directory.join("console-printk.state");
        let saved = read_snapshot(&snapshot, uid)?;
        let recovered = saved.is_some();
        let previous = match saved {
            Some(previous) => previous,
            None => {
                let mut current = String::new();
                File::open(printk)?.take(MAX_SNAPSHOT_BYTES + 1).read_to_string(&mut current)?;
                validate_printk(&current)?;
                write_snapshot(&snapshot, &current)?;
                current
            }
        };

        let guard = Self { printk: printk.to_path_buf(), snapshot, previous, recovered, _lease: lease };
        let mut levels = validate_printk(&guard.previous)?;
        levels[0] = "1";
        fs::write(printk, format!("{}\n", levels.join("\t")))?;
        Ok(guard)
    }
}

impl Drop for PrintkGuard {
    fn drop(&mut self) {
        if let Err(error) = fs::write(&self.printk, &self.previous) {
            tracing::warn!(%error, "failed to restore kernel console logging; retaining recovery snapshot");
            return;
        }
        if let Err(error) = fs::remove_file(&self.snapshot).and_then(|()| sync_directory(self.snapshot.parent().unwrap())) {
            tracing::warn!(%error, "failed to remove restored console logging snapshot");
        }
    }
}

fn acquire_lease(directory: &Path, uid: u32) -> io::Result<File> {
    match DirBuilder::new().mode(0o755).create(directory) {
        Ok(()) => {},
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
        return Err(unsafe_snapshot());
    }
    let lease = OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(directory.join("console-printk.lock"))?;
    verify_file(&lease, uid)?;
    let lock = libc::flock {
        l_type: libc::F_WRLCK as libc::c_short,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    };
    // Process-associated locks are not inherited by the forked PAM owner.
    // CLOEXEC alone would leave a flock held for the entire desktop session.
    // SAFETY: the descriptor and lock pointer are valid; F_SETLK never waits.
    if unsafe { libc::fcntl(lease.as_raw_fd(), libc::F_SETLK, &lock) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // Never unlink the lease: concurrent Greeters must use the same inode.
    Ok(lease)
}

fn read_snapshot(path: &Path, uid: u32) -> io::Result<Option<String>> {
    let file = match OpenOptions::new().read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    verify_file(&file, uid)?;
    let mut saved = String::new();
    file.take(MAX_SNAPSHOT_BYTES + 1).read_to_string(&mut saved)?;
    validate_printk(&saved)?;
    Ok(Some(saved))
}

fn verify_file(file: &File, uid: u32) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
        return Err(unsafe_snapshot());
    }
    Ok(())
}

fn validate_printk(text: &str) -> io::Result<Vec<&str>> {
    let levels: Vec<_> = text.split_whitespace().collect();
    if text.len() as u64 > MAX_SNAPSHOT_BYTES || levels.len() != 4
        || levels.iter().any(|level| level.parse::<u32>().is_err())
    {
        return Err(unsafe_snapshot());
    }
    Ok(levels)
}

fn write_snapshot(path: &Path, text: &str) -> io::Result<()> {
    let mut random = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = path.with_extension(format!("{suffix}.tmp"));
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&temporary)?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        sync_directory(path.parent().unwrap())
    })();
    if result.is_err() { let _ = fs::remove_file(temporary); }
    result
}

fn sync_directory(directory: &Path) -> io::Result<()> {
    OpenOptions::new().read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory)?.sync_all()
}

fn unsafe_snapshot() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "unsafe or invalid console logging snapshot")
}

/// systemd's documented PID 1 signals: SIGRTMIN+20 enables status, +21 disables.
fn set_systemd_console_status(enabled: bool) -> bool {
    let signal = libc::SIGRTMIN() + if enabled { 20 } else { 21 };
    // SAFETY: kill(2) with a valid signal targeting PID 1 has no memory effects.
    unsafe { libc::kill(1, signal) == 0 }
}

#[cfg(test)]
#[path = "console_log_tests.rs"]
mod tests;

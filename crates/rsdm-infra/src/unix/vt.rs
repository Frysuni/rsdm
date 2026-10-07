//! Exclusive virtual-terminal ownership for the greeter.
//!
//! A live foreign session is waited out; a second greeter is rejected before
//! either process can draw or receive keystrokes on the same VT.

use std::{
    fs::{File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VtError {
    #[error("could not restore terminal state on {path}: {source}")]
    Restore {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("virtual terminal {path} is already in use by another session")]
    Busy { path: String },
    #[error("another rsdm greeter already owns {path}")]
    AlreadyRunning { path: String },
    #[error("could not open {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: io::Error,
    },
}

/// A held claim on the greeter's VT. Keep it alive for as long as the greeter
/// runs; dropping it releases the single-instance lock.
pub struct VtGuard {
    /// Held open purely to keep the advisory `flock` for the lifetime of the
    /// greeter; closing the file releases the lock.
    _lock: Option<File>,
    tty_path: String,
}

impl VtGuard {
    /// Close the inherited lock descriptor in the forked session leader.
    /// Call this only in the child, before opening its PAM session.
    /// Only the greeter must keep the claim alive; unlocking explicitly would
    /// also release its lock because fork shares the open file description.
    pub fn release_in_session_child(&mut self) {
        self._lock.take();
    }

    /// Re-claim the VT foreground when control returns to the greeter after a
    /// session exits. While the session ran, the compositor became the VT's
    /// foreground; reclaiming makes the freshly redrawn greeter receive
    /// keystrokes again instead of a now-dead process group. Never reset a VT
    /// while a detached session still owns it.
    pub fn reclaim(&self) -> Result<(), VtError> {
        let file = open_vt(&self.tty_path).map_err(|source| VtError::Open {
            path: self.tty_path.clone(), source,
        })?;
        let rdev = file.metadata().map_err(|source| VtError::Open {
            path: self.tty_path.clone(), source,
        })?.rdev();
        wait_until_vt_is_free(file.as_raw_fd(), rdev, &self.tty_path)?;
        claim_foreground(file.as_raw_fd());
        restore_terminal(file.as_raw_fd(), &self.tty_path)
    }
}

/// Acquire exclusive ownership of `tty_path` for the greeter, or fail with a
/// reason the caller can surface without touching the terminal further.
pub fn acquire(tty_path: &str) -> Result<VtGuard, VtError> {
    let file = open_vt(tty_path).map_err(|source| VtError::Open {
        path: tty_path.to_string(),
        source,
    })?;
    let fd = file.as_raw_fd();

    // SAFETY: isatty only inspects the descriptor table.
    if unsafe { libc::isatty(fd) } != 1 {
        tracing::warn!(
            path = tty_path,
            "configured tty path is not a terminal; proceeding without VT ownership checks"
        );
    } else if !is_virtual_terminal(fd) {
        tracing::warn!(
            path = tty_path,
            "configured tty is not a Linux virtual terminal; proceeding anyway"
        );
    }

    let lock = match lock_vt(tty_path) {
        LockOutcome::Held(file) => Some(file),
        LockOutcome::Contended => {
            return Err(VtError::AlreadyRunning {
                path: tty_path.to_string(),
            });
        }
        LockOutcome::Unavailable => None,
    };

    let rdev = file.metadata().map_err(|source| VtError::Open {
        path: tty_path.to_string(), source,
    })?.rdev();
    wait_until_vt_is_free(fd, rdev, tty_path)?;

    claim_foreground(fd);
    restore_terminal(fd, tty_path)?;

    Ok(VtGuard {
        _lock: lock,
        tty_path: tty_path.to_string(),
    })
}

fn open_vt(tty_path: &str) -> io::Result<File> {
    // O_NOCTTY: opening the VT must not accidentally make it our controlling
    // terminal here; the greeter's controlling terminal is set up by systemd.
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(tty_path)
}

fn restore_terminal(fd: i32, tty_path: &str) -> Result<(), VtError> {
    // SAFETY: isatty only inspects the open descriptor. Skip descriptors with
    // no line discipline; a terminal needs a known baseline before raw mode.
    if unsafe { libc::isatty(fd) } != 1 { return Ok(()); }
    super::terminal::restore_cooked(fd).map_err(|source| VtError::Restore {
        path: tty_path.to_string(), source,
    })
}

/// `KDGKBTYPE` (get keyboard type) - a stable Linux console ioctl that succeeds
/// only on a virtual console. libc does not export the constant, so name it here.
const KDGKBTYPE: libc::c_ulong = 0x4B33;
/// `KDGETMODE` - read the console's KD mode (text or graphics).
const KDGETMODE: libc::c_ulong = 0x4B3B;
const KDSETMODE: libc::c_ulong = 0x4B3A;
const KD_GRAPHICS: libc::c_int = 1;
const KD_TEXT: libc::c_int = 0;

/// Graphics mode can outlive a crashed compositor. Check process ownership
/// separately before recovering it; controlling-TTY metadata alone cannot see
/// a compositor which detached through setsid.
fn vt_in_graphics_mode(fd: i32) -> bool {
    let mut mode: libc::c_int = 0;
    // SAFETY: KDGETMODE writes one int on a console fd and fails harmlessly
    // (without touching it) on any other descriptor.
    unsafe { libc::ioctl(fd, KDGETMODE, &mut mode) == 0 && mode == KD_GRAPHICS }
}

/// Wait until no live session owns the VT: neither a graphics-mode owner (the
/// compositor of a still-running login) nor a text session whose controlling
/// terminal it is (a console login). See the module docs for why this waits
/// instead of failing. A service stop during the wait aborts it.
fn wait_until_vt_is_free(fd: i32, rdev: u64, tty_path: &str) -> Result<(), VtError> {
    let mut waited = false;
    loop {
        let graphics = vt_in_graphics_mode(fd);
        if !super::vt_owner::is_busy(rdev) {
            if graphics {
                restore_text_mode(fd, tty_path)?;
            }
            if waited {
                tracing::info!(path = tty_path, "VT released; starting the greeter");
            }
            return Ok(());
        }
        if super::shutdown::terminate_requested() {
            tracing::info!(
                path = tty_path,
                "service stop requested while waiting for the VT"
            );
            return Err(VtError::Busy {
                path: tty_path.to_string(),
            });
        }
        if !waited {
            waited = true;
            tracing::info!(
                path = tty_path,
                graphics,
                "VT is owned by a live session; waiting for it to end instead of drawing over it"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn restore_text_mode(fd: i32, tty_path: &str) -> Result<(), VtError> {
    // SAFETY: fd is the acquired console. Only reset a graphics VT after the
    // process scan confirmed no foreign session or open VT descriptor remains.
    if unsafe { libc::ioctl(fd, KDSETMODE, KD_TEXT) } != 0 {
        return Err(VtError::Restore {
            path: tty_path.to_string(),
            source: io::Error::last_os_error(),
        });
    }
    tracing::info!(path = tty_path, "restored an unowned graphics VT to text mode");
    Ok(())
}

/// True when `fd` is a Linux virtual console (`KDGKBTYPE` succeeds only there).
fn is_virtual_terminal(fd: i32) -> bool {
    let mut kb_type: libc::c_char = 0;
    // SAFETY: KDGKBTYPE writes one byte to kb_type on a console fd and simply
    // fails (without touching it) on any other descriptor.
    unsafe { libc::ioctl(fd, KDGKBTYPE, &mut kb_type) == 0 }
}

/// Make the calling process group the VT's foreground so its keystrokes arrive
/// here. Only works when this VT is our controlling terminal; otherwise it fails
/// harmlessly and is ignored.
fn claim_foreground(fd: i32) {
    // tcsetpgrp from a background process group on the controlling terminal
    // raises SIGTTOU, whose default action would *stop* the greeter. Ignore it
    // across the call (the standard job-control idiom) and restore the previous
    // disposition afterwards.
    // SAFETY: signal/getpgrp/tcsetpgrp only adjust process and terminal state;
    // SIG_IGN is a valid handler value.
    unsafe {
        let previous = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        let pgrp = libc::getpgrp();
        if libc::tcsetpgrp(fd, pgrp) != 0 {
            let error = io::Error::last_os_error();
            tracing::debug!(%error, "could not claim VT foreground (VT is not our controlling terminal)");
        }
        libc::signal(libc::SIGTTOU, previous);
    }
}

enum LockOutcome {
    Held(File),
    Contended,
    Unavailable,
}

/// Take a non-blocking advisory lock keyed on the VT name so two greeters cannot
/// own the same terminal. The lock lives in the held [`File`]; closing it (when
/// the greeter exits) releases it, so a systemd restart re-acquires cleanly.
fn lock_vt(tty_path: &str) -> LockOutcome {
    let Some(name) = Path::new(tty_path)
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return LockOutcome::Unavailable;
    };
    let dir = Path::new("/run/rsdm");
    if let Err(error) = std::fs::create_dir_all(dir) {
        tracing::debug!(%error, "could not create /run/rsdm; skipping the single-instance lock");
        return LockOutcome::Unavailable;
    }
    let lock_path = dir.join(format!("{name}.lock"));
    let file = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(error) => {
            tracing::debug!(%error, path = %lock_path.display(), "could not open VT lock file; skipping the single-instance lock");
            return LockOutcome::Unavailable;
        }
    };
    // SAFETY: flock on a valid fd; LOCK_NB makes it return instead of blocking.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        LockOutcome::Held(file)
    } else if io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK) {
        LockOutcome::Contended
    } else {
        LockOutcome::Unavailable
    }
}

#[cfg(test)]
#[path = "vt_tests.rs"]
mod tests;

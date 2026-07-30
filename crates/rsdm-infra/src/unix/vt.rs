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
    /// Re-claim the VT foreground when control returns to the greeter after a
    /// session exits. While the session ran, the compositor became the VT's
    /// foreground; reclaiming makes the freshly redrawn greeter receive
    /// keystrokes again instead of a now-dead process group. Best-effort.
    pub fn reclaim(&self) {
        match open_vt(&self.tty_path) {
            Ok(file) => claim_foreground(file.as_raw_fd()),
            Err(error) => {
                tracing::debug!(path = %self.tty_path, %error, "could not reopen VT to reclaim foreground");
            }
        }
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

    let rdev = file.metadata().map(|meta| meta.rdev()).unwrap_or(0);
    wait_until_vt_is_free(fd, rdev, tty_path)?;

    claim_foreground(fd);

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

/// `KDGKBTYPE` (get keyboard type) - a stable Linux console ioctl that succeeds
/// only on a virtual console. libc does not export the constant, so name it here.
const KDGKBTYPE: libc::c_ulong = 0x4B33;
/// `KDGETMODE` - read the console's KD mode (text or graphics).
const KDGETMODE: libc::c_ulong = 0x4B3B;
const KD_GRAPHICS: libc::c_int = 1;

/// True when the VT is in `KD_GRAPHICS` mode - a compositor (or a boot splash)
/// currently owns its display. The controlling-terminal scan below cannot see
/// such an owner: the session-leader and the compositor both `setsid()` away
/// from the VT, so no process keeps it as controlling terminal while a Wayland
/// session runs on it. The mode is the reliable signal.
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
        if !graphics && !vt_is_busy(rdev) {
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

/// True when `fd` is a Linux virtual console (`KDGKBTYPE` succeeds only there).
fn is_virtual_terminal(fd: i32) -> bool {
    let mut kb_type: libc::c_char = 0;
    // SAFETY: KDGKBTYPE writes one byte to kb_type on a console fd and simply
    // fails (without touching it) on any other descriptor.
    unsafe { libc::ioctl(fd, KDGKBTYPE, &mut kb_type) == 0 }
}

/// Detect a foreign session by matching `/proc/<pid>/stat` controlling TTY.
fn vt_is_busy(rdev: u64) -> bool {
    let target = encode_tty_nr(rdev);
    if target == 0 {
        // No controlling-terminal device number to match (e.g. the path was not
        // a real VT); nothing to scan for.
        return false;
    }
    // SAFETY: getsid(0) queries our own session id and has no preconditions.
    let our_sid = unsafe { libc::getsid(0) };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        tracing::debug!("could not read /proc to check VT ownership; assuming free");
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{name}/stat")) else {
            continue; // Process vanished or is unreadable; ignore.
        };
        let Some((session, tty_nr)) = parse_session_and_tty(&stat) else {
            continue;
        };
        if tty_nr == target && session > 0 && session != our_sid {
            tracing::debug!(
                pid = name,
                session,
                our_sid,
                "found a foreign session on the VT; refusing to take it over"
            );
            return true;
        }
    }
    false
}

/// Encode a device number the way the kernel writes `tty_nr` in
/// `/proc/<pid>/stat` (`new_encode_dev`), so it can be compared directly. The
/// input `rdev` is the glibc-encoded `dev_t` from `stat(2)`.
fn encode_tty_nr(rdev: u64) -> i32 {
    let major = ((rdev >> 8) & 0x0000_0fff) | ((rdev >> 32) & !0x0000_0fffu64);
    let minor = (rdev & 0xff) | ((rdev >> 12) & !0xffu64);
    ((minor & 0xff) | (major << 8) | ((minor & !0xff) << 12)) as i32
}

/// Parse `(session_id, tty_nr)` from the contents of `/proc/<pid>/stat`. The
/// `comm` field (2nd) is wrapped in parentheses and may itself contain spaces or
/// parentheses, so split after the last `)`: the remaining fields are
/// `state ppid pgrp session tty_nr ...`.
fn parse_session_and_tty(stat: &str) -> Option<(i32, i32)> {
    let close = stat.rfind(')')?;
    let mut fields = stat.get(close + 1..)?.split_whitespace();
    let _state = fields.next()?;
    let _ppid = fields.next()?;
    let _pgrp = fields.next()?;
    let session = fields.next()?.parse().ok()?;
    let tty_nr = fields.next()?.parse().ok()?;
    Some((session, tty_nr))
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
mod tests {
    use super::*;

    /// glibc `makedev(major, minor)` - the encoding `stat(2)` returns in `rdev`.
    fn makedev(major: u64, minor: u64) -> u64 {
        ((major & 0xfff) << 8)
            | ((major & !0xfffu64) << 32)
            | (minor & 0xff)
            | ((minor & !0xffu64) << 12)
    }

    #[test]
    fn encode_tty_nr_matches_kernel_for_virtual_consoles() {
        // /dev/tty1 is char 4:1; the kernel writes tty_nr = 1025 in stat.
        assert_eq!(encode_tty_nr(makedev(4, 1)), 1025);
        // /dev/tty2 is 4:2 -> 1026, /dev/tty12 is 4:12 -> 1036.
        assert_eq!(encode_tty_nr(makedev(4, 2)), 1026);
        assert_eq!(encode_tty_nr(makedev(4, 12)), 1036);
    }

    #[test]
    fn encode_tty_nr_handles_large_minor_numbers() {
        // A minor that spills past 8 bits must round-trip through both halves of
        // the kernel encoding (e.g. a serial console, 4:300).
        let major = 4u64;
        let minor = 300u64;
        let encoded = encode_tty_nr(makedev(major, minor)) as u64;
        let decoded_minor = (encoded & 0xff) | ((encoded >> 12) & !0xffu64);
        let decoded_major = (encoded >> 8) & 0xfff;
        assert_eq!(decoded_major, major);
        assert_eq!(decoded_minor, minor);
    }

    #[test]
    fn parse_session_and_tty_reads_the_right_fields() {
        // pid (comm) state ppid pgrp session tty_nr tpgid ...
        let stat = "1234 (bash) S 1000 1234 1234 1025 1300 ...";
        assert_eq!(parse_session_and_tty(stat), Some((1234, 1025)));
    }

    #[test]
    fn parse_session_and_tty_survives_parens_and_spaces_in_comm() {
        // comm can contain spaces and parentheses; only the last ')' delimits it.
        let stat = "42 (weird )(name) R 1 7 9 1026 -1 0";
        assert_eq!(parse_session_and_tty(stat), Some((9, 1026)));
    }

    #[test]
    fn parse_session_and_tty_rejects_garbage() {
        assert_eq!(parse_session_and_tty("not a stat line"), None);
        assert_eq!(parse_session_and_tty("123 (x) S 1"), None);
    }
}

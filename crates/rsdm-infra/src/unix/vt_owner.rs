//! Process liveness checks for virtual-terminal ownership.

use std::{io, os::unix::fs::MetadataExt};

/// Detached compositors and logind may hold the VT without a controlling TTY.
pub(super) fn is_busy(rdev: u64) -> bool {
    let target = encode_tty_nr(rdev);
    if target == 0 {
        // No controlling-terminal device number to match (e.g. the path was not
        // a real VT); nothing to scan for.
        return false;
    }
    // SAFETY: getsid(0) queries our own session id and has no preconditions.
    let our_sid = unsafe { libc::getsid(0) };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        tracing::warn!("could not read /proc to check VT ownership; assuming busy");
        return true;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return true;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        if process_owns_tty(name, our_sid, target, rdev) {
            return true;
        }
    }
    false
}

fn process_owns_tty(pid: &str, our_sid: i32, target: i32, rdev: u64) -> bool {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) => return error.kind() != io::ErrorKind::NotFound,
    };
    let Some((session, tty_nr)) = parse_session_and_tty(&stat) else {
        return true;
    };
    if session == our_sid {
        return false;
    }
    if tty_nr == target || has_tty_descriptor(pid, rdev) {
        tracing::debug!(pid, session, "found a foreign process holding the VT");
        return true;
    }
    false
}

fn has_tty_descriptor(pid: &str, rdev: u64) -> bool {
    let entries = match std::fs::read_dir(format!("/proc/{pid}/fd")) {
        Ok(entries) => entries,
        Err(error) => return error.kind() != io::ErrorKind::NotFound,
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return true;
        };
        match std::fs::metadata(entry.path()) {
            Ok(metadata) if metadata.rdev() == rdev => return true,
            Err(error) if error.kind() != io::ErrorKind::NotFound => return true,
            _ => {}
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

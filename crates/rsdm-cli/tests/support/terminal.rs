//! Capture a real CLI terminal and reject screen/input control sequences.

use std::{fs, io::{self, Read}, os::fd::{FromRawFd, OwnedFd}, process::{Command, ExitStatus, Stdio}};

pub fn terminal(mut command: Command, columns: u16) -> (ExitStatus, String) {
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize { ws_row: 24, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: fd outputs and size are live storage; null optional name/termios are supported.
    assert_eq!(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &size) }, 0);
    // SAFETY: openpty returned two fresh descriptors, each transferred to exactly one owner.
    let mut reader = fs::File::from(unsafe { OwnedFd::from_raw_fd(master) });
    // SAFETY: slave is the second owned descriptor from openpty.
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    command.stdout(Stdio::from(slave.try_clone().unwrap())).stderr(Stdio::from(slave));
    let mut child = command.spawn().unwrap();
    drop(command);
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => bytes.extend_from_slice(&chunk[..count]),
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => panic!("reading PTY: {error}"),
        }
    }
    (child.wait().unwrap(), String::from_utf8(bytes).unwrap())
}

pub fn assert_only_styles(text: &str) {
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            assert_eq!(chars.next(), Some('['));
            loop {
                match chars.next().expect("complete escape") {
                    '0'..='9' | ';' => {}
                    'm' => break,
                    other => panic!("unexpected terminal control {other:?}"),
                }
            }
        }
    }
}


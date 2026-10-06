//! Bounded, interruptible I/O for the Greeter/session-leader pipes.

use std::{io, os::raw::c_int};

pub(super) fn read_frame(fd: c_int) -> Option<[u8; 5]> {
    let mut frame = [0u8; 5];
    read_exact(fd, &mut frame).ok()?;
    Some(frame)
}

pub(super) fn read_exact(fd: c_int, buffer: &mut [u8]) -> io::Result<()> {
    let mut filled = 0;
    while filled < buffer.len() {
        wait_for_data(fd)?;
        // SAFETY: the unfilled tail is a valid writable slice.
        let read = unsafe {
            libc::read(fd, buffer[filled..].as_mut_ptr().cast(), buffer.len() - filled)
        };
        if read > 0 {
            filled += read as usize;
        } else if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        } else if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn wait_for_data(fd: c_int) -> io::Result<()> {
    let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    loop {
        if super::shutdown::terminate_requested() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        // SAFETY: poll references one initialized entry; the timeout avoids a
        // lost SIGTERM immediately before the blocking call starts.
        let result = unsafe { libc::poll(&mut poll, 1, 200) };
        if result > 0 {
            return Ok(());
        }
        if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error());
        }
    }
}

pub(super) fn write_frame(fd: c_int, bytes: &[u8]) -> io::Result<()> {
    let mut written = 0;
    while written < bytes.len() {
        // SAFETY: bytes is a valid readable slice until write returns.
        let wrote = unsafe {
            libc::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written)
        };
        if wrote > 0 {
            written += wrote as usize;
        } else {
            let error = io::Error::last_os_error();
            if wrote < 0 && error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
    }
    Ok(())
}

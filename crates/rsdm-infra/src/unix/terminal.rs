//! A cooked terminal baseline for an exclusively owned Greeter or fallback VT.

use std::io;

pub(super) fn restore_cooked(fd: i32) -> io::Result<()> {
    // SAFETY: tcgetattr initializes the local termios for an open terminal.
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
        return Err(io::Error::last_os_error());
    }

    termios.c_iflag |= libc::ICRNL | libc::IXON | libc::BRKINT;
    termios.c_iflag &= !(libc::IGNBRK | libc::INLCR | libc::IGNCR | libc::ISTRIP | libc::IXOFF);
    termios.c_oflag |= libc::OPOST | libc::ONLCR;
    termios.c_lflag |= libc::ISIG | libc::ICANON | libc::ECHO | libc::ECHOE | libc::ECHOK | libc::IEXTEN;
    termios.c_cflag &= !(libc::CSIZE | libc::PARENB);
    termios.c_cflag |= libc::CREAD | libc::CS8;

    termios.c_cc[libc::VINTR] = 3;
    termios.c_cc[libc::VQUIT] = 28;
    termios.c_cc[libc::VERASE] = 127;
    termios.c_cc[libc::VKILL] = 21;
    termios.c_cc[libc::VEOF] = 4;
    termios.c_cc[libc::VSUSP] = 26;
    termios.c_cc[libc::VMIN] = 1;
    termios.c_cc[libc::VTIME] = 0;

    // SAFETY: fd refers to the terminal the caller has claimed. Do not flush
    // pending input or reset the device when restoring its line discipline.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;

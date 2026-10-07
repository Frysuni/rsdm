//! Terminal recovery uses private PTYs; no active console is changed.

use std::{
    fs::File, io::{Read, Write}, os::{fd::{AsRawFd, FromRawFd}, unix::net::UnixStream},
    time::{Duration, Instant},
};

use super::*;

struct Pty { master: File, slave: File }

impl Pty {
    fn new() -> Self {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: descriptor outputs are writable and optional arguments may be null.
        assert_eq!(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null()) }, 0);
        // SAFETY: openpty returned two descriptors; each File owns exactly one.
        let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
        // SAFETY: slave is open. Nonblocking reads keep failed assertions finite.
        assert_eq!(unsafe { libc::fcntl(slave.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) }, 0);
        Self { master, slave }
    }

    fn attributes(&self) -> libc::termios {
        // SAFETY: tcgetattr initializes this termios from our PTY descriptor.
        let mut attributes = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::tcgetattr(self.slave.as_raw_fd(), &mut attributes) }, 0);
        attributes
    }

    fn read_line(&mut self) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut buffer = [0; 128];
        loop {
            match self.slave.read(&mut buffer) {
                Ok(count) => return buffer[..count].to_vec(),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                other => panic!("PTY line read failed: {other:?}"),
            }
        }
    }
}

struct Child(libc::pid_t);

impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: this is our unreaped fixture child; kill and reap it even
        // after a failed assertion. It performs no uninterruptible I/O.
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
            libc::waitpid(self.0, std::ptr::null_mut(), 0);
        }
    }
}

#[test]
fn a_hard_killed_raw_terminal_owner_is_restored_before_the_next_greeter() {
    let mut pty = Pty::new();
    let fd = pty.slave.as_raw_fd();
    let mut raw = pty.attributes();
    // SAFETY: cfmakeraw changes only the local initialized termios.
    unsafe { libc::cfmakeraw(&mut raw) };
    let (mut parent, child) = UnixStream::pair().unwrap();
    parent.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    // SAFETY: the child uses only libc terminal/descriptor calls and _exit,
    // without allocation or re-entering the multithreaded test harness.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        unsafe {
            libc::close(parent.as_raw_fd());
            if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 { libc::_exit(1); }
            if libc::write(child.as_raw_fd(), [1u8].as_ptr().cast(), 1) != 1 { libc::_exit(1); }
            loop { libc::pause(); }
        }
    }
    let owner = Child(pid);
    drop(child);
    let mut ready = [0];
    parent.read_exact(&mut ready).unwrap();
    assert_eq!(ready, [1]);
    drop(owner);
    assert_eq!(pty.attributes().c_lflag & (libc::ICANON | libc::ECHO), 0);

    restore_cooked(fd).unwrap();
    let cooked = pty.attributes();
    assert_ne!(cooked.c_lflag & libc::ECHO, 0);
    assert_ne!(cooked.c_lflag & libc::ISIG, 0);
    assert_eq!(cooked.c_cc[libc::VINTR], 3);
    pty.master.write_all(b"typing").unwrap();
    assert_eq!(pty.slave.read(&mut ready).unwrap_err().kind(), io::ErrorKind::WouldBlock);
    pty.master.write_all(b"\r").unwrap();
    assert_eq!(pty.read_line(), b"typing\n");
}

#[test]
fn restoring_line_discipline_preserves_device_speed() {
    let pty = Pty::new();
    let before = pty.attributes();
    restore_cooked(pty.slave.as_raw_fd()).unwrap();
    let after = pty.attributes();
    // SAFETY: these queries read initialized local termios structures.
    unsafe {
        assert_eq!(libc::cfgetispeed(&before), libc::cfgetispeed(&after));
        assert_eq!(libc::cfgetospeed(&before), libc::cfgetospeed(&after));
    }
}

#[test]
fn invalid_terminal_descriptors_are_reported() {
    assert!(restore_cooked(-1).is_err());
}

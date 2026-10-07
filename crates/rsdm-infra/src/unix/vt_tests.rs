use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::*;

#[test]
fn session_child_does_not_retain_or_unlock_the_greeter_claim() {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("rsdm-vt-lock-{}-{nonce}", std::process::id()));
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact", "unix::vt::tests::session_child_lock_probe",
            "--ignored", "--nocapture", "--test-threads=1",
        ])
        .env("RSDM_TEST_VT_LOCK", &path)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(path);

    assert!(output.status.success(), "{}{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
#[ignore = "isolated subprocess fixture for the VT lock regression"]
fn session_child_lock_probe() {
    let path = std::env::var_os("RSDM_TEST_VT_LOCK").unwrap();
    let lock = OpenOptions::new().create_new(true).write(true).open(&path).unwrap();
    assert_eq!(try_lock(&lock), 0);
    let mut guard = VtGuard { _lock: Some(lock), tty_path: String::new() };
    let (mut parent, child) = UnixStream::pair().unwrap();
    parent.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    // SAFETY: the child only closes inherited descriptors, communicates over
    // the socket using libc, and exits without re-entering the test harness.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        run_session_child(&mut guard, parent.as_raw_fd(), child.as_raw_fd());
    }
    let child_process = SessionChild(pid);
    drop(child);
    let mut ready = [0];
    parent.read_exact(&mut ready).unwrap();
    assert_eq!(ready, [1], "the session child's lock descriptor must be closed");

    let next = OpenOptions::new().write(true).open(path).unwrap();
    let result = try_lock(&next);
    let error = io::Error::last_os_error();
    assert_eq!(result, -1, "the live greeter must retain its lock");
    assert_eq!(error.raw_os_error(), Some(libc::EWOULDBLOCK));
    drop(guard);
    assert_eq!(try_lock(&next), 0, "a living session child must not block a new greeter");
    parent.write_all(&[1]).unwrap();
    child_process.wait();
}

fn try_lock(file: &File) -> libc::c_int {
    // SAFETY: file owns a valid descriptor and the operation never blocks.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }
}

fn run_session_child(guard: &mut VtGuard, parent_fd: libc::c_int, child_fd: libc::c_int) -> ! {
    let lock_fd = guard._lock.as_ref().unwrap().as_raw_fd();
    guard.release_in_session_child();

    // SAFETY: the socket endpoints were opened before fork. Only this child's
    // descriptor table is changed; the greeter keeps its own lock descriptor.
    unsafe {
        libc::close(parent_fd);
        let mut ready = [u8::from(libc::fcntl(lock_fd, libc::F_GETFD) == -1)];
        if libc::write(child_fd, ready.as_ptr().cast(), 1) != 1 {
            libc::_exit(1);
        }
        let read = libc::read(child_fd, ready.as_mut_ptr().cast(), 1);
        libc::_exit(if read == 1 && ready == [1] { 0 } else { 1 });
    }
}

struct SessionChild(libc::pid_t);

impl SessionChild {
    fn wait(mut self) {
        let mut status = 0;
        // SAFETY: this is our still-owned child and status is writable.
        assert_eq!(unsafe { libc::waitpid(self.0, &mut status, 0) }, self.0);
        self.0 = 0;
        assert!(libc::WIFEXITED(status));
        assert_eq!(libc::WEXITSTATUS(status), 0);
    }
}

impl Drop for SessionChild {
    fn drop(&mut self) {
        if self.0 == 0 {
            return;
        }
        // SAFETY: terminate and reap only our child, including after a failed
        // assertion, so the test never leaves a blocked subprocess behind.
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
            libc::waitpid(self.0, std::ptr::null_mut(), 0);
        }
    }
}

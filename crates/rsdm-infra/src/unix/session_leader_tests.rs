//! Cancellation only waits for or signals children created by these fixtures.

use std::{io::Read, process::{Command, Stdio}};

use super::*;
use rsdm_system::process_handle::ProcessHandle;

#[test]
fn cancellation_allows_the_owner_to_finish_cleanup_after_eof() {
    let mut child = Command::new("sh")
        .args(["-c", "cat >/dev/null; sleep 0.2; printf closed"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut output = child.stdout.take().unwrap();
    let pid = child.id() as libc::pid_t;
    drop(child.stdin.take());

    // The session-leader waitpid calls, rather than Child::wait, own this exit.
    terminate(pid);
    reap(pid);
    let mut result = String::new();
    output.read_to_string(&mut result).unwrap();
    assert_eq!(result, "closed");
}

#[test]
fn cancellation_still_kills_an_unresponsive_owner() {
    let child = Command::new("sh").args(["-c", "kill -STOP $$"]).spawn().unwrap();
    let pid = child.id() as libc::pid_t;
    let handle = ProcessHandle::open(child.id()).unwrap().unwrap();
    let started = Instant::now();
    terminate(pid);
    reap(pid);
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(!handle.alive().unwrap());
}

//! Deadline checks signal only children launched by these private fixtures.

use std::{io::{BufRead, BufReader}, os::{fd::OwnedFd, unix::net::UnixStream}, thread};

use super::*;
use crate::unix::process_handle::ProcessHandle;

#[test]
fn a_foreground_command_preserves_its_nonzero_exit_status() {
    let mut command = Command::new("sh");
    command.args(["-c", "exit 7"]);
    let status = run_command_until(&mut command, Instant::now() + Duration::from_secs(2)).unwrap();
    assert_eq!(status.code(), Some(7));
}

#[test]
fn expired_deadlines_never_launch_a_command() {
    let mut command = Command::new("/a/program/that/must/not/be/executed");
    let error = run_command_until(&mut command, Instant::now()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
}

#[test]
fn timeout_kills_stopped_leaders_and_shell_descendants() {
    for script in [
        "printf '%s\\n' $$; kill -STOP $$; exit 0",
        "trap '' TERM; sleep 30 & printf '%s\\n' \"$!\"; wait",
    ] {
        let (reader, writer) = UnixStream::pair().unwrap();
        reader.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", script]).stdout(OwnedFd::from(writer));
        let before = Instant::now();
        let worker = thread::spawn(move || run_command_until(&mut command, before + Duration::from_secs(1)));
        let mut pid = String::new();
        BufReader::new(reader).read_line(&mut pid).unwrap();
        let handle = ProcessHandle::open(pid.trim().parse().unwrap()).unwrap().unwrap();
        let result = worker.join().unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(before.elapsed() < Duration::from_secs(2));
        let exited_by = Instant::now() + Duration::from_secs(1);
        while handle.alive().unwrap() && Instant::now() < exited_by {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!handle.alive().unwrap(), "timeout left the recorded child or descendant alive");
    }
}

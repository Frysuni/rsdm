use std::io::{BufRead, BufReader};
use std::process::Stdio;

use super::*;

struct Launcher(Child);

impl Launcher {
    fn new(ignore_term: bool) -> Self {
        let script = if ignore_term {
            "trap '' TERM; printf 'ready\\n'; exec sleep 30"
        } else {
            "printf 'ready\\n'; exec sleep 30"
        };
        let mut child = Command::new("sh").args(["-c", script]).stdout(Stdio::piped()).spawn().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut ready).unwrap();
        assert_eq!(ready, "ready\n");
        Self(child)
    }

    fn status(&mut self) -> ExitStatus {
        assert!(async_io::block_on(wait_for_launcher(&mut self.0, Duration::from_secs(1))).unwrap());
        self.0.try_wait().unwrap().unwrap()
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn assert_timed_out(result: Result<(), SessionError>) {
    assert!(matches!(result, Err(SessionError::Bus(error)) if super::super::bus::retryable_error(&error)));
}

#[test]
fn an_ordinary_launcher_is_terminated_and_reaped() {
    let mut launcher = Launcher::new(false);
    async_io::block_on(stop_launcher(&mut launcher.0, &Deadline::default())).unwrap();
    assert_eq!(launcher.status().signal(), Some(libc::SIGTERM));
}

#[test]
fn an_already_reaped_launcher_needs_no_shutdown_budget() {
    let mut launcher = Launcher::new(false);
    launcher.0.kill().unwrap();
    let status = launcher.status();
    let deadline = Deadline::default();
    deadline.set(1);
    async_io::block_on(stop_launcher(&mut launcher.0, &deadline)).unwrap();
    assert_eq!(launcher.status(), status);
}

#[test]
fn an_expired_budget_skips_grace_and_kills_only_the_owned_launcher() {
    let mut launcher = Launcher::new(true);
    let mut unrelated = Launcher::new(true);
    let deadline = Deadline::default();
    deadline.set(1);
    let before = Instant::now();
    assert_timed_out(async_io::block_on(stop_launcher(&mut launcher.0, &deadline)));
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(launcher.status().signal(), Some(libc::SIGKILL));
    assert!(unrelated.0.try_wait().unwrap().is_none());
}

#[test]
fn revising_the_deadline_interrupts_an_active_launcher_wait() {
    let mut launcher = Launcher::new(true);
    let deadline = Deadline::default();
    let mut stopping = Box::pin(stop_launcher(&mut launcher.0, &deadline));
    assert!(async_io::block_on(futures_lite::future::poll_once(stopping.as_mut())).is_none());
    deadline.set(1);
    let before = Instant::now();
    assert_timed_out(async_io::block_on(stopping));
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(launcher.status().signal(), Some(libc::SIGKILL));
}

#[test]
fn escalation_does_not_add_a_new_wait_after_the_shared_budget() {
    let mut launcher = Launcher::new(true);
    let deadline = Deadline::default();
    deadline.set(super::super::processes::monotonic_usec().unwrap() + 50_000);
    let before = Instant::now();
    assert_timed_out(async_io::block_on(stop_launcher(&mut launcher.0, &deadline)));
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(launcher.status().signal(), Some(libc::SIGKILL));
}

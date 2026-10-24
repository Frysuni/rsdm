use std::{future::Future, pin::Pin, process::Stdio, task::{Context, Waker}, time::Instant};

use tokio::io::{AsyncBufReadExt, BufReader};

use super::*;

#[path = "session_process_exit_tests.rs"]
mod exit;

struct Launcher(Child);

impl Launcher {
    async fn new(ignore_term: bool) -> Self {
        let script = if ignore_term {
            "trap '' TERM; printf 'ready\\n'; exec sleep 30"
        } else {
            "printf 'ready\\n'; exec sleep 30"
        };
        let mut child = Command::new("sh").args(["-c", script]).stdout(Stdio::piped()).spawn().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut ready).await.unwrap();
        assert_eq!(ready, "ready\n");
        Self(child)
    }

    async fn status(&mut self) -> ExitStatus {
        assert!(wait_for_launcher(&mut self.0, Duration::from_secs(1)).await.unwrap());
        self.0.try_wait().unwrap().unwrap()
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

fn assert_pending(future: Pin<&mut impl Future>) {
    assert!(future.poll(&mut Context::from_waker(Waker::noop())).is_pending());
}

fn assert_timed_out(result: Result<(), SessionError>) {
    assert!(matches!(result, Err(SessionError::Bus(error)) if super::super::bus::retryable_error(&error)));
}

#[tokio::test]
async fn an_ordinary_launcher_is_terminated_and_reaped() {
    let mut launcher = Launcher::new(false).await;
    stop_launcher(&mut launcher.0, &Deadline::default(), ActionGate::default()).await.unwrap();
    assert_eq!(launcher.status().await.signal(), Some(libc::SIGTERM));
}

#[tokio::test]
async fn an_already_reaped_launcher_needs_no_shutdown_budget() {
    let mut launcher = Launcher::new(false).await;
    launcher.0.kill().await.unwrap();
    let status = launcher.status().await;
    let deadline = Deadline::default();
    deadline.set(1);
    stop_launcher(&mut launcher.0, &deadline, ActionGate::default()).await.unwrap();
    assert_eq!(launcher.status().await, status);
}

#[tokio::test]
async fn an_expired_budget_skips_grace_and_kills_only_the_owned_launcher() {
    let mut launcher = Launcher::new(true).await;
    let mut unrelated = Launcher::new(true).await;
    let deadline = Deadline::default();
    deadline.set(1);
    let before = Instant::now();
    assert_timed_out(stop_launcher(&mut launcher.0, &deadline, ActionGate::default()).await);
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(launcher.status().await.signal(), Some(libc::SIGKILL));
    assert!(unrelated.0.try_wait().unwrap().is_none());
    unrelated.0.kill().await.unwrap();
}

#[tokio::test]
async fn revising_the_deadline_interrupts_an_active_launcher_wait() {
    let mut launcher = Launcher::new(true).await;
    let deadline = Deadline::default();
    let mut stopping = Box::pin(stop_launcher(&mut launcher.0, &deadline, ActionGate::default()));
    assert_pending(stopping.as_mut());
    deadline.set(1);
    let before = Instant::now();
    assert_timed_out(stopping.await);
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(launcher.status().await.signal(), Some(libc::SIGKILL));
}

#[tokio::test]
async fn escalation_does_not_add_a_new_wait_after_the_shared_budget() {
    let mut launcher = Launcher::new(true).await;
    let deadline = Deadline::default();
    deadline.set(super::super::processes::monotonic_usec().unwrap() + 50_000);
    let before = Instant::now();
    assert_timed_out(stop_launcher(&mut launcher.0, &deadline, ActionGate::default()).await);
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(launcher.status().await.signal(), Some(libc::SIGKILL));
}

#[tokio::test]
async fn cancelling_a_pending_wait_keeps_the_child_available_for_owned_escalation() {
    let mut launcher = Launcher::new(true).await;
    let pid = launcher.0.id().unwrap();
    let mut waiting = Box::pin(wait_for_launcher(&mut launcher.0, Duration::from_secs(5)));
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(20)) => {},
        result = &mut waiting => panic!("launcher wait completed before cancellation: {result:?}"),
    }
    drop(waiting);
    assert_eq!(launcher.0.id(), Some(pid));
    assert!(launcher.0.try_wait().unwrap().is_none());

    let deadline = Deadline::default();
    deadline.set(1);
    assert_timed_out(stop_launcher(&mut launcher.0, &deadline, ActionGate::default()).await);
    assert_eq!(launcher.status().await.signal(), Some(libc::SIGKILL));
}

#[tokio::test]
async fn disabling_during_term_grace_keeps_the_launcher_alive_without_kill() {
    let mut launcher = Launcher::new(true).await;
    let actions = ActionGate::default();
    let deadline = Deadline::default();
    let mut stopping = Box::pin(stop_launcher(&mut launcher.0, &deadline, actions.clone()));
    assert_pending(stopping.as_mut());
    actions.close();
    let result = tokio::time::timeout(Duration::from_secs(1), stopping).await.unwrap();
    assert!(matches!(result, Err(SessionError::Disabled)));
    assert!(launcher.0.try_wait().unwrap().is_none());
    launcher.0.kill().await.unwrap();
}

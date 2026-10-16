use std::{os::unix::net::UnixStream, process::Command};

use crate::auth_protocol::{self, Frame};
use super::*;

#[test]
fn closed_helper_connection_terminates_the_process_and_limits_retries() {
    let (parent, mut peer) = UnixStream::pair().unwrap();
    let child = Command::new("sleep").arg("2").spawn().unwrap();
    let fixture = std::thread::spawn(move || {
        let start = auth_protocol::read_frame(&mut peer).unwrap();
        assert!(matches!(start, Frame::Start { username, service, password }
            if username == "alice" && service == "test" && password.is_empty()));
        // A helper can close its connection while remaining alive in PAM
        // teardown. EOF must not turn into an unbounded process wait.
    });
    let limiter = Arc::new(MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60)));
    let (_responses_tx, responses) = mpsc::sync_channel(1);
    let (events, _events_rx) = mpsc::sync_channel(1);
    let before = Instant::now();
    let deadline = before + Duration::from_secs(1);
    let helper = HelperProcess { child: Some(child), deadline };
    let mut exchange = Exchange {
        stream: parent, responses, events, cancel: Arc::new(AtomicBool::new(false)), deadline,
    };
    let outcome = run_helper(helper, &mut exchange, "alice", "test", PasswordSecret::new(""), &limiter);
    fixture.join().unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(matches!(outcome, Attempt::Failed(_)));

    let job = start_authentication(
        PasswordSecret::new(""), "alice".into(), "test".into(), limiter,
    ).unwrap();
    assert!(matches!(job.events.try_recv(), Ok(AuthEvent::Finished(Attempt::RateLimited(_)))));
}

#[test]
fn helper_result_does_not_wait_for_process_exit_and_resets_prior_failures() {
    let (parent, mut peer) = UnixStream::pair().unwrap();
    let child = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id();
    let fixture = std::thread::spawn(move || {
        let start = auth_protocol::read_frame(&mut peer).unwrap();
        assert!(matches!(start, Frame::Start { password, .. } if password.is_empty()));
        auth_protocol::write_frame(&mut peer, &Frame::Finished {
            success: true, message: String::new(),
        }).unwrap();
    });
    let limiter = MemoryLoginAttemptLimiter::new(2, Duration::from_secs(60));
    limiter.record_failure("alice");
    let (_responses_tx, responses) = mpsc::sync_channel(1);
    let (events, _events_rx) = mpsc::sync_channel(1);
    let before = Instant::now();
    let deadline = before + Duration::from_secs(1);
    let helper = HelperProcess { child: Some(child), deadline };
    let mut exchange = Exchange {
        stream: parent, responses, events, cancel: Arc::new(AtomicBool::new(false)), deadline,
    };
    let outcome = run_helper(helper, &mut exchange, "alice", "test", PasswordSecret::new(""), &limiter);
    fixture.join().unwrap();
    assert_eq!(outcome, Attempt::Unlocked);
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    limiter.record_failure("alice");
    assert!(limiter.check_allowed("alice").is_ok());
}

#[test]
fn exited_helper_output_is_drained_before_deciding_the_result() {
    let (parent, mut peer) = UnixStream::pair().unwrap();
    let mut child = Command::new("true").spawn().unwrap();
    child.wait().unwrap();
    auth_protocol::write_frame(&mut peer, &Frame::Message(AuthMessage {
        style: rsdm_core::ports::AuthMessageStyle::Info,
        text: "x".repeat(12 * 1024),
    })).unwrap();
    auth_protocol::write_frame(&mut peer, &Frame::Finished {
        success: true, message: String::new(),
    }).unwrap();
    let limiter = MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60));
    let (_responses_tx, responses) = mpsc::sync_channel(1);
    let (events, events_rx) = mpsc::sync_channel(1);
    let deadline = Instant::now() + Duration::from_secs(1);
    let helper = HelperProcess { child: Some(child), deadline };
    let mut exchange = Exchange {
        stream: parent, responses, events, cancel: Arc::new(AtomicBool::new(false)), deadline,
    };
    let outcome = run_helper(helper, &mut exchange, "alice", "test", PasswordSecret::new(""), &limiter);
    assert_eq!(outcome, Attempt::Unlocked);
    assert!(matches!(events_rx.try_recv(), Ok(AuthEvent::Message(message))
        if message.text.len() == 12 * 1024));
}

#[test]
fn a_stalled_helper_times_out_and_is_terminated() {
    let (parent, _peer) = UnixStream::pair().unwrap();
    let child = Command::new("sleep").arg("30").spawn().unwrap();
    let limiter = MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60));
    let (_responses_tx, responses) = mpsc::sync_channel(1);
    let (events, _events_rx) = mpsc::sync_channel(1);
    let before = Instant::now();
    let deadline = before + Duration::from_millis(100);
    let helper = HelperProcess { child: Some(child), deadline };
    let mut exchange = Exchange {
        stream: parent, responses, events, cancel: Arc::new(AtomicBool::new(false)), deadline,
    };
    let outcome = run_helper(helper, &mut exchange, "alice", "test", PasswordSecret::new(""), &limiter);
    assert_eq!(outcome, Attempt::Failed("Authentication timed out".into()));
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(limiter.check_allowed("alice").is_err());
}

#[test]
fn dropping_a_job_cancels_its_worker_without_consuming_a_rate_limit_attempt() {
    let (parent, _peer) = UnixStream::pair().unwrap();
    let child = Command::new("sleep").arg("30").spawn().unwrap();
    let limiter = MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60));
    let (responses_tx, responses) = mpsc::sync_channel(1);
    let (events_tx, events) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let job = AuthenticationJob {
        events, responses: responses_tx, waiting: false, cancelled: false, cancel: Arc::clone(&cancel),
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    let helper = HelperProcess { child: Some(child), deadline };
    let mut exchange = Exchange { stream: parent, responses, events: events_tx, cancel, deadline };
    drop(job);
    let outcome = run_helper(helper, &mut exchange, "alice", "test", PasswordSecret::new(""), &limiter);
    assert_eq!(outcome, Attempt::Failed("Authentication cancelled".into()));
    assert!(limiter.check_allowed("alice").is_ok());
}

#[test]
fn a_stalled_ui_cannot_bypass_the_deadline_with_a_queued_success() {
    let (parent, mut peer) = UnixStream::pair().unwrap();
    let child = Command::new("sleep").arg("30").spawn().unwrap();
    let message = AuthMessage { style: rsdm_core::ports::AuthMessageStyle::Info, text: "notice".into() };
    auth_protocol::write_frame(&mut peer, &Frame::Message(message)).unwrap();
    auth_protocol::write_frame(&mut peer, &Frame::Finished { success: true, message: String::new() }).unwrap();
    let limiter = MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60));
    let (_responses_tx, responses) = mpsc::sync_channel(1);
    let (events, _events_rx) = mpsc::sync_channel(0);
    let before = Instant::now();
    let deadline = before + Duration::from_millis(50);
    let helper = HelperProcess { child: Some(child), deadline };
    let mut exchange = Exchange {
        stream: parent, responses, events, cancel: Arc::new(AtomicBool::new(false)), deadline,
    };
    let outcome = run_helper(helper, &mut exchange, "alice", "test", PasswordSecret::new(""), &limiter);
    assert_eq!(outcome, Attempt::Failed("Authentication timed out".into()));
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(limiter.check_allowed("alice").is_err());
}

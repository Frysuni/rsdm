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
            if username == "alice" && service == "test" && password.expose_secret().is_empty()));
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
        credentials_sent: false,
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
        assert!(matches!(start, Frame::Start { password, .. } if password.expose_secret().is_empty()));
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
        credentials_sent: false,
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
        credentials_sent: false,
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
        credentials_sent: false,
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
    let mut exchange = Exchange { credentials_sent: false, stream: parent, responses, events: events_tx, cancel, deadline };
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
        credentials_sent: false,
        stream: parent, responses, events, cancel: Arc::new(AtomicBool::new(false)), deadline,
    };
    let outcome = run_helper(helper, &mut exchange, "alice", "test", PasswordSecret::new(""), &limiter);
    assert_eq!(outcome, Attempt::Failed("Authentication timed out".into()));
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(limiter.check_allowed("alice").is_err());
}

#[test]
fn cancelling_a_submitted_password_still_limits_retries() {
    let (parent, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let child = Command::new("sleep").arg("30").spawn().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let peer_cancel = Arc::clone(&cancel);
    let (completed, wait) = mpsc::channel();
    let fixture = std::thread::spawn(move || {
        assert!(matches!(auth_protocol::read_frame(&mut peer).unwrap(),
            Frame::Start { password, .. } if password.expose_secret() == "wrong"));
        peer_cancel.store(true, Ordering::Release);
        wait.recv_timeout(Duration::from_secs(2)).unwrap();
    });
    let limiter = MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60));
    let (_responses_tx, responses) = mpsc::sync_channel(1);
    let (events, _events_rx) = mpsc::sync_channel(1);
    let deadline = Instant::now() + Duration::from_secs(1);
    let helper = HelperProcess { child: Some(child), deadline };
    let mut exchange = Exchange { credentials_sent: false, stream: parent, responses, events, cancel, deadline };

    let outcome = run_helper(helper, &mut exchange, "alice", "test", PasswordSecret::new("wrong"), &limiter);
    completed.send(()).unwrap();
    fixture.join().unwrap();
    assert_eq!(outcome, Attempt::Failed("Authentication cancelled".into()));
    assert!(limiter.check_allowed("alice").is_err());
}

#[test]
fn broken_helper_protocol_cannot_unlock_or_avoid_the_failure_budget() {
    use std::io::Write;

    for failure in ["unexpected-answer", "oversized-frame", "truncated-success"] {
        let (parent, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let fixture = std::thread::spawn(move || {
            assert!(matches!(auth_protocol::read_frame(&mut peer).unwrap(),
                Frame::Start { password, .. } if password.expose_secret() == "submitted"));
            let mut success = Vec::new();
            auth_protocol::write_frame(&mut success, &Frame::Finished { success: true, message: String::new() }).unwrap();
            let mut output = Vec::new();
            match failure {
                "unexpected-answer" => {
                    auth_protocol::write_frame(&mut output, &Frame::Answer(None)).unwrap();
                    output.extend_from_slice(&success);
                }
                "oversized-frame" => {
                    output.extend_from_slice(&u32::MAX.to_be_bytes());
                    output.extend_from_slice(&success);
                }
                "truncated-success" => output.extend_from_slice(&success[..success.len() - 1]),
                _ => unreachable!(),
            }
            peer.write_all(&output).unwrap();
        });
        let limiter = Arc::new(MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60)));
        let (_responses_tx, responses) = mpsc::sync_channel(1);
        let (events, events_rx) = mpsc::sync_channel(1);
        let deadline = Instant::now() + Duration::from_secs(2);
        let helper = HelperProcess { child: Some(child), deadline };
        let mut exchange = Exchange {
            credentials_sent: false, stream: parent, responses, events,
            cancel: Arc::new(AtomicBool::new(false)), deadline,
        };
        let outcome = run_helper(helper, &mut exchange, "alice", "test", PasswordSecret::new("submitted"), &limiter);
        fixture.join().unwrap();
        assert_eq!(outcome, Attempt::Failed("Authentication failed".into()), "{failure}");
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        assert!(events_rx.try_recv().is_err());
        assert!(limiter.check_allowed("bob").is_ok());
        let next = start_authentication(PasswordSecret::new("retry"), "alice".into(), "test".into(), limiter).unwrap();
        assert!(matches!(next.events.try_recv(), Ok(AuthEvent::Finished(Attempt::RateLimited(_)))));
    }
}

#[test]
fn cancelling_a_second_pam_prompt_rejects_late_success_and_reaps_the_helper() {
    use rsdm_core::ports::AuthMessageStyle;

    let (parent, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let child = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id();
    let (release, cancelled) = mpsc::channel();
    let fixture = std::thread::spawn(move || {
        assert!(matches!(auth_protocol::read_frame(&mut peer).unwrap(),
            Frame::Start { password, .. } if password.expose_secret() == "submitted"));
        for (style, text) in [(AuthMessageStyle::Info, "notice"), (AuthMessageStyle::Secret, "first factor")] {
            auth_protocol::write_frame(&mut peer, &Frame::Message(AuthMessage { style, text: text.into() })).unwrap();
        }
        assert!(matches!(auth_protocol::read_frame(&mut peer).unwrap(),
            Frame::Answer(Some(answer)) if answer.expose_secret() == "factor"));
        auth_protocol::write_frame(&mut peer, &Frame::Message(AuthMessage {
            style: AuthMessageStyle::Visible, text: "second factor".into(),
        })).unwrap();
        cancelled.recv_timeout(Duration::from_secs(2)).unwrap();
        // The helper may finish before it sees the cancellation or connection closure.
        let _ = auth_protocol::write_frame(&mut peer, &Frame::Finished { success: true, message: String::new() });
        assert!(auth_protocol::read_frame(&mut peer).is_err());
    });

    let limiter = Arc::new(MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60)));
    let worker_limiter = Arc::clone(&limiter);
    let cancel = Arc::new(AtomicBool::new(false));
    let (responses, responses_rx) = mpsc::sync_channel(1);
    let (events_tx, events) = mpsc::sync_channel(1);
    let mut job = AuthenticationJob { events, responses, waiting: false, cancelled: false, cancel: Arc::clone(&cancel) };
    let deadline = Instant::now() + Duration::from_secs(3);
    let worker = std::thread::spawn(move || {
        let helper = HelperProcess { child: Some(child), deadline };
        let mut exchange = Exchange {
            credentials_sent: false, stream: parent, responses: responses_rx, events: events_tx,
            cancel, deadline,
        };
        run_helper(helper, &mut exchange, "alice", "test", PasswordSecret::new("submitted"), &worker_limiter)
    });

    for (style, text) in [(AuthMessageStyle::Info, "notice"), (AuthMessageStyle::Secret, "first factor")] {
        assert!(matches!(job.events.recv_timeout(Duration::from_secs(2)).unwrap(),
            AuthEvent::Message(message) if message.style == style && message.text == text));
    }
    job.responses.send(Ok(PasswordSecret::new("factor"))).unwrap();
    assert!(matches!(job.events.recv_timeout(Duration::from_secs(2)).unwrap(),
        AuthEvent::Message(message) if message.style == AuthMessageStyle::Visible && message.text == "second factor"));
    job.cancel();
    release.send(()).unwrap();
    assert_eq!(worker.join().unwrap(), Attempt::Failed("Authentication cancelled".into()));
    fixture.join().unwrap();
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    assert!(limiter.check_allowed("alice").is_err());
}

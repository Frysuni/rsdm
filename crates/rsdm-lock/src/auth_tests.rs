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
    let outcome = run_helper(
        child, parent, "alice".into(), "test".into(), PasswordSecret::new(""),
        responses, Arc::new(AtomicBool::new(false)), Arc::clone(&limiter), events,
    );
    fixture.join().unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(matches!(outcome, Attempt::Failed(_)));

    let job = start_authentication(
        PasswordSecret::new(""), "alice".into(), "test".into(), limiter,
    ).unwrap();
    assert!(matches!(job.events.try_recv(), Ok(AuthEvent::Finished(Attempt::RateLimited(_)))));
}

#[test]
fn helper_can_accept_an_empty_password_and_reset_prior_failures() {
    let (parent, mut peer) = UnixStream::pair().unwrap();
    let child = Command::new("sleep").arg("0.2").spawn().unwrap();
    let fixture = std::thread::spawn(move || {
        let start = auth_protocol::read_frame(&mut peer).unwrap();
        assert!(matches!(start, Frame::Start { password, .. } if password.is_empty()));
        auth_protocol::write_frame(&mut peer, &Frame::Finished {
            success: true, message: String::new(),
        }).unwrap();
    });
    let limiter = Arc::new(MemoryLoginAttemptLimiter::new(2, Duration::from_secs(60)));
    limiter.record_failure("alice");
    let (_responses_tx, responses) = mpsc::sync_channel(1);
    let (events, _events_rx) = mpsc::sync_channel(1);
    let outcome = run_helper(
        child, parent, "alice".into(), "test".into(), PasswordSecret::new(""),
        responses, Arc::new(AtomicBool::new(false)), Arc::clone(&limiter), events,
    );
    fixture.join().unwrap();
    assert_eq!(outcome, Attempt::Unlocked);
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
    let limiter = Arc::new(MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60)));
    let (_responses_tx, responses) = mpsc::sync_channel(1);
    let (events, events_rx) = mpsc::sync_channel(1);
    let outcome = run_helper(
        child, parent, "alice".into(), "test".into(), PasswordSecret::new(""),
        responses, Arc::new(AtomicBool::new(false)), limiter, events,
    );
    assert_eq!(outcome, Attempt::Unlocked);
    assert!(matches!(events_rx.try_recv(), Ok(AuthEvent::Message(message))
        if message.text.len() == 12 * 1024));
}

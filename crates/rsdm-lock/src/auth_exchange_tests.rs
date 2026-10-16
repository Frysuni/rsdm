use rsdm_core::ports::{AuthMessage, AuthMessageStyle};

use super::*;

fn notice() -> AuthEvent {
    AuthEvent::Message(AuthMessage { style: AuthMessageStyle::Info, text: "notice".into() })
}

#[test]
fn a_full_event_queue_obeys_the_authentication_deadline() {
    let (events, _receiver) = mpsc::sync_channel(1);
    events.send(notice()).unwrap();
    let before = Instant::now();
    let error = send_event(&events, notice(), &AtomicBool::new(false), before + Duration::from_millis(50)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(before.elapsed() < Duration::from_secs(1));
}

#[test]
fn cancellation_interrupts_a_full_event_queue() {
    let (events, _receiver) = mpsc::sync_channel(1);
    events.send(notice()).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let worker = thread::spawn(move || {
        send_event(&events, notice(), &worker_cancel, Instant::now() + Duration::from_secs(30))
    });
    cancel.store(true, Ordering::Release);
    assert_eq!(worker.join().unwrap().unwrap_err().kind(), io::ErrorKind::Interrupted);
}

#[test]
fn a_terminal_result_can_be_delivered_at_the_deadline() {
    let (events, receiver) = mpsc::sync_channel(1);
    send_event(&events, AuthEvent::Finished(crate::auth::Attempt::Failed("timeout".into())),
        &AtomicBool::new(false), Instant::now()).unwrap();
    assert!(matches!(receiver.try_recv(), Ok(AuthEvent::Finished(_))));
}

#[test]
fn socket_backpressure_is_bounded_and_partial_writes_preserve_bytes() {
    for peer_reads in [false, true] {
        let (stream, mut peer) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        let (_responses_tx, responses) = mpsc::sync_channel(1);
        let (events, _events_rx) = mpsc::sync_channel(1);
        let timeout = if peer_reads { Duration::from_secs(2) } else { Duration::from_millis(50) };
        let before = Instant::now();
        let mut exchange = Exchange {
            stream, responses, events, cancel: Arc::new(AtomicBool::new(false)), deadline: before + timeout,
        };
        let bytes = vec![42; 1024 * 1024];
        let reader = thread::spawn(move || {
            if !peer_reads {
                thread::sleep(Duration::from_millis(200));
                return Vec::new();
            }
            let mut received = Vec::new();
            peer.read_to_end(&mut received).unwrap();
            received
        });
        let result = exchange.write_all(&bytes);
        drop(exchange);
        let received = reader.join().unwrap();
        if peer_reads {
            result.unwrap();
            assert_eq!(received, bytes);
        } else {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
            assert!(before.elapsed() < Duration::from_secs(1));
        }
    }
}

#[test]
fn cancelled_exchange_does_not_write_credentials() {
    let (stream, mut peer) = UnixStream::pair().unwrap();
    peer.set_nonblocking(true).unwrap();
    let (_responses_tx, responses) = mpsc::sync_channel(1);
    let (events, _events_rx) = mpsc::sync_channel(1);
    let mut exchange = Exchange {
        stream, responses, events, cancel: Arc::new(AtomicBool::new(true)),
        deadline: Instant::now() + Duration::from_secs(1),
    };
    let error = exchange.run("alice", "test", PasswordSecret::new("secret")).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    assert_eq!(peer.read(&mut [0; 1]).unwrap_err().kind(), io::ErrorKind::WouldBlock);
}

#[test]
fn successive_prompts_receive_their_own_answers() {
    let (stream, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let (responses_tx, responses) = mpsc::sync_channel(1);
    let (events, events_rx) = mpsc::sync_channel(1);
    let helper = thread::spawn(move || {
        assert!(matches!(auth_protocol::read_frame(&mut peer).unwrap(), Frame::Start { .. }));
        for (style, answer) in [(AuthMessageStyle::Secret, "password"), (AuthMessageStyle::Visible, "otp")] {
            auth_protocol::write_frame(&mut peer, &Frame::Message(AuthMessage { style, text: answer.into() })).unwrap();
            assert!(matches!(auth_protocol::read_frame(&mut peer).unwrap(), Frame::Answer(Some(value)) if value == answer));
        }
        auth_protocol::write_frame(&mut peer, &Frame::Finished { success: true, message: String::new() }).unwrap();
    });
    let ui = thread::spawn(move || {
        for expected in ["password", "otp"] {
            let AuthEvent::Message(message) = events_rx.recv_timeout(Duration::from_secs(2)).unwrap() else {
                panic!("expected a PAM prompt");
            };
            assert_eq!(message.text, expected);
            responses_tx.send(Ok(PasswordSecret::new(expected))).unwrap();
        }
        // Keep the response sender alive until the worker finishes reading.
        assert!(events_rx.recv_timeout(Duration::from_secs(2)).is_err());
    });
    let mut exchange = Exchange {
        stream, responses, events, cancel: Arc::new(AtomicBool::new(false)),
        deadline: Instant::now() + Duration::from_secs(2),
    };
    let result = exchange.run("alice", "test", PasswordSecret::new(""));
    drop(exchange);
    helper.join().unwrap();
    ui.join().unwrap();
    assert!(result.unwrap());
}

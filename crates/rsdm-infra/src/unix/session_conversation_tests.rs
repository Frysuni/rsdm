use super::*;
use std::{os::unix::net::UnixStream, time::Duration};

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(1)
}

#[derive(Debug)]
struct Cancel;

impl AuthConversation for Cancel {
    fn respond(&mut self, _: AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        Err(AuthError::InvalidCredentials)
    }
}

#[test]
fn cancelled_conversation_cannot_authorize_a_session() {
    let (report, child_report) = UnixStream::pair().unwrap();
    let (reply, child_reply) = UnixStream::pair().unwrap();
    send_text(child_report.as_raw_fd(), SECRET_TAG, b"OTP:", None).unwrap();
    write_frame(child_report.as_raw_fd(), &[super::super::session_report::AUTHORIZED_TAG, 0, 0, 0, 0]).unwrap();
    assert_eq!(read_auth_report(report.as_raw_fd(), reply.as_raw_fd(), &mut Cancel, deadline()),
        Some(super::super::session_report::LeaderReport::AuthFailed.encode()));
    assert_eq!(read_frame(child_reply.as_raw_fd()).unwrap(), [CANCEL_TAG, 0, 0, 0, 0]);
}

#[test]
fn oversized_messages_are_rejected_before_reading_or_writing() {
    assert_eq!(send_text(-1, SECRET_TAG, &vec![0; MAX_PASSWORD_BYTES + 1], None).unwrap_err().kind(), io::ErrorKind::InvalidData);
    let length = (MAX_PASSWORD_BYTES as u32 + 1).to_le_bytes();
    let frame = [ANSWER_TAG, length[0], length[1], length[2], length[3]];
    assert_eq!(read_text(-1, frame).unwrap_err().kind(), io::ErrorKind::InvalidData);
}

#[test]
fn authorization_wait_expires_without_a_child_frame() {
    let (report, _child_report) = UnixStream::pair().unwrap();
    let (reply, _child_reply) = UnixStream::pair().unwrap();
    let started = Instant::now();
    assert!(read_auth_report(
        report.as_raw_fd(),
        reply.as_raw_fd(),
        &mut Cancel,
        started + Duration::from_millis(20),
    ).is_none());
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[derive(Debug, Default)]
struct AccountConversation {
    accounts: Vec<String>,
    denied: bool,
}

impl AuthConversation for AccountConversation {
    fn respond(&mut self, _: AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        Err(AuthError::InvalidCredentials)
    }

    fn account_name(&mut self, username: &str) -> Result<(), AuthError> {
        self.accounts.push(username.to_string());
        if self.denied { return Err(AuthError::AccountDenied); }
        Ok(())
    }
}

#[test]
fn canonical_account_is_acknowledged_before_session_authorization() {
    for denied in [false, true] {
        let (report, child_report) = UnixStream::pair().unwrap();
        let (reply, child_reply) = UnixStream::pair().unwrap();
        let child = std::thread::spawn(move || {
            let mut conversation = LeaderConversation { report: child_report.into(), reply: child_reply.into() };
            let result = conversation.account_name("alice");
            assert_eq!(result.is_err(), denied);
            // Even a malformed child cannot bypass a denied account by
            // claiming authorization after the parent rejected its name.
            write_frame(conversation.report.as_raw_fd(), &[super::super::session_report::AUTHORIZED_TAG, 0, 0, 0, 0]).unwrap();
        });
        let mut conversation = AccountConversation { denied, ..Default::default() };
        let result = read_auth_report(report.as_raw_fd(), reply.as_raw_fd(), &mut conversation, deadline()).unwrap();
        child.join().unwrap();
        assert_eq!(conversation.accounts, ["alice"]);
        let expected = if denied { super::super::session_report::LeaderReport::AuthFailed.encode() }
            else { [super::super::session_report::AUTHORIZED_TAG, 0, 0, 0, 0] };
        assert_eq!(result, expected);
    }
}

#[test]
fn account_mapping_after_cancellation_does_not_allow_a_session() {
    let (report, child_report) = UnixStream::pair().unwrap();
    let (reply, child_reply) = UnixStream::pair().unwrap();
    send_text(child_report.as_raw_fd(), SECRET_TAG, b"OTP:", None).unwrap();
    send_text(child_report.as_raw_fd(), ACCOUNT_TAG, b"alice", None).unwrap();
    write_frame(child_report.as_raw_fd(), &[super::super::session_report::AUTHORIZED_TAG, 0, 0, 0, 0]).unwrap();
    let mut conversation = AccountConversation::default();
    assert_eq!(read_auth_report(report.as_raw_fd(), reply.as_raw_fd(), &mut conversation, deadline()),
        Some(super::super::session_report::LeaderReport::AuthFailed.encode()));
    assert_eq!(conversation.accounts, ["alice"]);
    for _ in 0..2 { assert_eq!(read_frame(child_reply.as_raw_fd()).unwrap(), [CANCEL_TAG, 0, 0, 0, 0]); }
}

#[test]
fn invalid_account_reports_are_rejected() {
    for username in [String::new(), "a\0b".into(), "a".repeat(MAX_USERNAME_BYTES + 1)] {
        assert_eq!(validate_account(&username).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
    let length = (MAX_USERNAME_BYTES as u32 + 1).to_le_bytes();
    let frame = [ACCOUNT_TAG, length[0], length[1], length[2], length[3]];
    let error = receive_account(-1, -1, frame, &mut AccountConversation::default(), &mut false, deadline()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn failed_authentication_reports_its_mapped_account() {
    let (report, child_report) = UnixStream::pair().unwrap();
    let (reply, child_reply) = UnixStream::pair().unwrap();
    send_text(child_report.as_raw_fd(), ACCOUNT_TAG, b"alice", None).unwrap();
    let failure = super::super::session_report::LeaderReport::AuthFailed.encode();
    write_frame(child_report.as_raw_fd(), &failure).unwrap();
    let mut conversation = AccountConversation::default();
    assert_eq!(read_auth_report(report.as_raw_fd(), reply.as_raw_fd(), &mut conversation, deadline()), Some(failure));
    assert_eq!(conversation.accounts, ["alice"]);
    assert_eq!(read_frame(child_reply.as_raw_fd()).unwrap(), [ACCOUNT_ACCEPTED_TAG, 0, 0, 0, 0]);
}

#[derive(Debug)]
struct Answer(bool);

impl AuthConversation for Answer {
    fn respond(&mut self, _: AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        if self.0 { Ok(Some(PasswordSecret::new("answer"))) }
        else { Err(AuthError::InvalidCredentials) }
    }
}

#[test]
fn stalled_reply_readers_cannot_extend_the_authorization_deadline() {
    use std::{io::Write, sync::mpsc, thread};

    for (tag, accepts) in [(SECRET_TAG, true), (SECRET_TAG, false), (ACCOUNT_TAG, true)] {
        let (report, child_report) = UnixStream::pair().unwrap();
        let (mut reply, child_reply) = UnixStream::pair().unwrap();
        reply.set_nonblocking(true).unwrap();
        loop {
            match reply.write(&[0; 4096]) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("filling the reply socket: {error}"),
            }
        }
        reply.set_nonblocking(false).unwrap();
        send_text(child_report.as_raw_fd(), tag, b"alice", None).unwrap();
        let (completed, result) = mpsc::channel();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_millis(50);
            let report = read_auth_report(report.as_raw_fd(), reply.as_raw_fd(), &mut Answer(accepts), deadline);
            completed.send(report).unwrap();
        });
        let report = result.recv_timeout(Duration::from_millis(200));
        // Release even a broken blocking writer before asserting the deadline.
        drop(child_reply);
        worker.join().unwrap();
        assert_eq!(report.unwrap(), None);
    }
}

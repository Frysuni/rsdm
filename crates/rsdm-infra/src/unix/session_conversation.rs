//! Interactive PAM messages relayed between the session leader and Greeter.

use std::{
    io,
    os::fd::{AsRawFd, OwnedFd},
};

use rsdm_core::{
    domain::PasswordSecret,
    ports::{AuthConversation, AuthError, AuthMessage, AuthMessageStyle, MAX_PASSWORD_BYTES, MAX_USERNAME_BYTES},
};
use zeroize::Zeroizing;

use super::session_pipe::{read_exact, read_frame, write_frame};

const SECRET_TAG: u8 = 8;
const VISIBLE_TAG: u8 = 9;
const INFO_TAG: u8 = 10;
const ERROR_TAG: u8 = 11;
const ANSWER_TAG: u8 = 12;
const CANCEL_TAG: u8 = 13;
const ACCOUNT_TAG: u8 = 14;
const ACCOUNT_ACCEPTED_TAG: u8 = 15;

#[derive(Debug)]
pub(super) struct LeaderConversation {
    pub report: OwnedFd,
    pub reply: OwnedFd,
}

impl AuthConversation for LeaderConversation {
    fn respond(&mut self, message: AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        let tag = match message.style {
            AuthMessageStyle::Secret => SECRET_TAG,
            AuthMessageStyle::Visible => VISIBLE_TAG,
            AuthMessageStyle::Info => INFO_TAG,
            AuthMessageStyle::Error => ERROR_TAG,
        };
        send_text(self.report.as_raw_fd(), tag, message.text.as_bytes()).map_err(conversation_error)?;
        if matches!(message.style, AuthMessageStyle::Info | AuthMessageStyle::Error) {
            return Ok(None);
        }
        let frame = read_frame(self.reply.as_raw_fd())
            .ok_or_else(|| conversation_error(io::ErrorKind::UnexpectedEof.into()))?;
        if frame[0] != ANSWER_TAG {
            return Err(AuthError::InvalidCredentials);
        }
        let bytes = read_text(self.reply.as_raw_fd(), frame).map_err(conversation_error)?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| AuthError::Backend("PAM response is not UTF-8".to_string()))?;
        Ok(Some(PasswordSecret::new(text)))
    }

    fn account_name(&mut self, username: &str) -> Result<(), AuthError> {
        validate_account(username).map_err(conversation_error)?;
        send_text(self.report.as_raw_fd(), ACCOUNT_TAG, username.as_bytes()).map_err(conversation_error)?;
        match read_frame(self.reply.as_raw_fd()) {
            Some([ACCOUNT_ACCEPTED_TAG, 0, 0, 0, 0]) => Ok(()),
            _ => Err(AuthError::AccountDenied),
        }
    }
}

pub(super) fn read_auth_report(
    report_fd: i32,
    reply_fd: i32,
    conversation: &mut dyn AuthConversation,
) -> Option<[u8; 5]> {
    let mut cancelled = false;
    loop {
        let Some(frame) = read_frame(report_fd) else {
            return cancelled.then(|| super::session_report::LeaderReport::AuthFailed.encode());
        };
        if cancelled && frame[0] == super::session_report::AUTHORIZED_TAG {
            return Some(super::session_report::LeaderReport::AuthFailed.encode());
        }
        if frame[0] == ACCOUNT_TAG {
            receive_account(report_fd, reply_fd, frame, conversation, &mut cancelled).ok()?;
            continue;
        }
        let style = match frame[0] {
            SECRET_TAG => AuthMessageStyle::Secret,
            VISIBLE_TAG => AuthMessageStyle::Visible,
            INFO_TAG => AuthMessageStyle::Info,
            ERROR_TAG => AuthMessageStyle::Error,
            _ => return Some(frame),
        };
        let bytes = read_text(report_fd, frame).ok()?;
        let text = std::str::from_utf8(&bytes).ok()?.to_string();
        let answer = if cancelled {
            Err(AuthError::InvalidCredentials)
        } else {
            conversation.respond(AuthMessage { style, text })
        };
        if matches!(style, AuthMessageStyle::Secret | AuthMessageStyle::Visible) {
            cancelled |= !matches!(&answer, Ok(Some(_)));
            send_answer(reply_fd, answer).ok()?;
        } else if answer.is_err() && !cancelled {
            return None;
        }
    }
}

fn receive_account(
    report_fd: i32, reply_fd: i32, frame: [u8; 5],
    conversation: &mut dyn AuthConversation, cancelled: &mut bool,
) -> io::Result<()> {
    let length = u32::from_le_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
    if length > MAX_USERNAME_BYTES { return Err(io::ErrorKind::InvalidData.into()); }
    let bytes = read_text(report_fd, frame)?;
    let username = std::str::from_utf8(&bytes).map_err(|_| io::ErrorKind::InvalidData)?;
    validate_account(username)?;
    // Keep mapping failed/cancelled attempts to the real account, but never
    // let an accepted mapping undo conversation cancellation.
    *cancelled |= conversation.account_name(username).is_err();
    let tag = if *cancelled { CANCEL_TAG } else { ACCOUNT_ACCEPTED_TAG };
    write_frame(reply_fd, &[tag, 0, 0, 0, 0])
}

fn validate_account(username: &str) -> io::Result<()> {
    if username.is_empty() || username.len() > MAX_USERNAME_BYTES || username.contains('\0') {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(())
}

fn send_answer(fd: i32, answer: Result<Option<PasswordSecret>, AuthError>) -> io::Result<()> {
    match answer {
        Ok(Some(answer)) => send_text(fd, ANSWER_TAG, answer.expose_secret().as_bytes()),
        _ => write_frame(fd, &[CANCEL_TAG, 0, 0, 0, 0]),
    }
}

fn send_text(fd: i32, tag: u8, text: &[u8]) -> io::Result<()> {
    if text.len() > MAX_PASSWORD_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let length = (text.len() as u32).to_le_bytes();
    write_frame(fd, &[tag, length[0], length[1], length[2], length[3]])?;
    write_frame(fd, text)
}

fn read_text(fd: i32, frame: [u8; 5]) -> io::Result<Zeroizing<Vec<u8>>> {
    let length = u32::from_le_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
    if length > MAX_PASSWORD_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut bytes = Zeroizing::new(vec![0; length]);
    read_exact(fd, &mut bytes)?;
    Ok(bytes)
}

fn conversation_error(error: io::Error) -> AuthError {
    AuthError::Backend(format!("PAM conversation pipe: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

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
        send_text(child_report.as_raw_fd(), SECRET_TAG, b"OTP:").unwrap();
        write_frame(child_report.as_raw_fd(), &[super::super::session_report::AUTHORIZED_TAG, 0, 0, 0, 0]).unwrap();
        assert_eq!(read_auth_report(report.as_raw_fd(), reply.as_raw_fd(), &mut Cancel),
            Some(super::super::session_report::LeaderReport::AuthFailed.encode()));
        assert_eq!(read_frame(child_reply.as_raw_fd()).unwrap(), [CANCEL_TAG, 0, 0, 0, 0]);
    }

    #[test]
    fn oversized_messages_are_rejected_before_reading_or_writing() {
        assert_eq!(send_text(-1, SECRET_TAG, &vec![0; MAX_PASSWORD_BYTES + 1]).unwrap_err().kind(), io::ErrorKind::InvalidData);
        let length = (MAX_PASSWORD_BYTES as u32 + 1).to_le_bytes();
        let frame = [ANSWER_TAG, length[0], length[1], length[2], length[3]];
        assert_eq!(read_text(-1, frame).unwrap_err().kind(), io::ErrorKind::InvalidData);
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
            let result = read_auth_report(report.as_raw_fd(), reply.as_raw_fd(), &mut conversation).unwrap();
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
        send_text(child_report.as_raw_fd(), SECRET_TAG, b"OTP:").unwrap();
        send_text(child_report.as_raw_fd(), ACCOUNT_TAG, b"alice").unwrap();
        write_frame(child_report.as_raw_fd(), &[super::super::session_report::AUTHORIZED_TAG, 0, 0, 0, 0]).unwrap();
        let mut conversation = AccountConversation::default();
        assert_eq!(read_auth_report(report.as_raw_fd(), reply.as_raw_fd(), &mut conversation),
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
        let error = receive_account(-1, -1, frame, &mut AccountConversation::default(), &mut false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn failed_authentication_reports_its_mapped_account() {
        let (report, child_report) = UnixStream::pair().unwrap();
        let (reply, child_reply) = UnixStream::pair().unwrap();
        send_text(child_report.as_raw_fd(), ACCOUNT_TAG, b"alice").unwrap();
        let failure = super::super::session_report::LeaderReport::AuthFailed.encode();
        write_frame(child_report.as_raw_fd(), &failure).unwrap();
        let mut conversation = AccountConversation::default();
        assert_eq!(read_auth_report(report.as_raw_fd(), reply.as_raw_fd(), &mut conversation), Some(failure));
        assert_eq!(conversation.accounts, ["alice"]);
        assert_eq!(read_frame(child_reply.as_raw_fd()).unwrap(), [ACCOUNT_ACCEPTED_TAG, 0, 0, 0, 0]);
    }
}

//! Interactive PAM messages relayed between the session leader and Greeter.

use std::{
    io,
    os::fd::{AsRawFd, OwnedFd},
    time::Instant,
};

use rsdm_core::{
    domain::PasswordSecret,
    ports::{AuthConversation, AuthError, AuthMessage, AuthMessageStyle, MAX_PASSWORD_BYTES, MAX_USERNAME_BYTES},
};
use zeroize::Zeroizing;

use super::session_pipe::{read_exact, read_exact_until, read_frame, read_frame_until, write_frame, write_frame_until};

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
        send_text(self.report.as_raw_fd(), tag, message.text.as_bytes(), None).map_err(conversation_error)?;
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
        send_text(self.report.as_raw_fd(), ACCOUNT_TAG, username.as_bytes(), None).map_err(conversation_error)?;
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
    deadline: Instant,
) -> Option<[u8; 5]> {
    let mut cancelled = false;
    loop {
        let Some(frame) = read_frame_until(report_fd, deadline) else {
            return cancelled.then(|| super::session_report::LeaderReport::AuthFailed.encode());
        };
        if cancelled && frame[0] == super::session_report::AUTHORIZED_TAG {
            return Some(super::session_report::LeaderReport::AuthFailed.encode());
        }
        if frame[0] == ACCOUNT_TAG {
            receive_account(report_fd, reply_fd, frame, conversation, &mut cancelled, deadline).ok()?;
            continue;
        }
        let style = match frame[0] {
            SECRET_TAG => AuthMessageStyle::Secret,
            VISIBLE_TAG => AuthMessageStyle::Visible,
            INFO_TAG => AuthMessageStyle::Info,
            ERROR_TAG => AuthMessageStyle::Error,
            _ => return Some(frame),
        };
        let bytes = read_text_until(report_fd, frame, deadline).ok()?;
        let text = std::str::from_utf8(&bytes).ok()?.to_string();
        let answer = if cancelled {
            Err(AuthError::InvalidCredentials)
        } else {
            conversation.respond(AuthMessage { style, text })
        };
        if matches!(style, AuthMessageStyle::Secret | AuthMessageStyle::Visible) {
            cancelled |= !matches!(&answer, Ok(Some(_)));
            send_answer(reply_fd, answer, deadline).ok()?;
        } else if answer.is_err() && !cancelled {
            return None;
        }
    }
}

fn receive_account(
    report_fd: i32, reply_fd: i32, frame: [u8; 5],
    conversation: &mut dyn AuthConversation, cancelled: &mut bool, deadline: Instant,
) -> io::Result<()> {
    let length = u32::from_le_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
    if length > MAX_USERNAME_BYTES { return Err(io::ErrorKind::InvalidData.into()); }
    let bytes = read_text_until(report_fd, frame, deadline)?;
    let username = std::str::from_utf8(&bytes).map_err(|_| io::ErrorKind::InvalidData)?;
    validate_account(username)?;
    // Keep mapping failed/cancelled attempts to the real account, but never
    // let an accepted mapping undo conversation cancellation.
    *cancelled |= conversation.account_name(username).is_err();
    let tag = if *cancelled { CANCEL_TAG } else { ACCOUNT_ACCEPTED_TAG };
    write_frame_until(reply_fd, &[tag, 0, 0, 0, 0], deadline)
}

fn validate_account(username: &str) -> io::Result<()> {
    if username.is_empty() || username.len() > MAX_USERNAME_BYTES || username.contains('\0') {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(())
}

fn send_answer(fd: i32, answer: Result<Option<PasswordSecret>, AuthError>, deadline: Instant) -> io::Result<()> {
    match answer {
        Ok(Some(answer)) => send_text(fd, ANSWER_TAG, answer.expose_secret().as_bytes(), Some(deadline)),
        _ => write_frame_until(fd, &[CANCEL_TAG, 0, 0, 0, 0], deadline),
    }
}

fn send_text(fd: i32, tag: u8, text: &[u8], deadline: Option<Instant>) -> io::Result<()> {
    if text.len() > MAX_PASSWORD_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let length = (text.len() as u32).to_le_bytes();
    let header = [tag, length[0], length[1], length[2], length[3]];
    for bytes in [header.as_slice(), text] {
        match deadline {
            Some(deadline) => write_frame_until(fd, bytes, deadline)?,
            None => write_frame(fd, bytes)?,
        }
    }
    Ok(())
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

fn read_text_until(fd: i32, frame: [u8; 5], deadline: Instant) -> io::Result<Zeroizing<Vec<u8>>> {
    let length = u32::from_le_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
    if length > MAX_PASSWORD_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut bytes = Zeroizing::new(vec![0; length]);
    read_exact_until(fd, &mut bytes, deadline)?;
    Ok(bytes)
}

fn conversation_error(error: io::Error) -> AuthError {
    AuthError::Backend(format!("PAM conversation pipe: {error}"))
}

#[cfg(test)]
#[path = "session_conversation_tests.rs"]
mod tests;

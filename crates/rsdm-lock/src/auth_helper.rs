use std::{io, os::fd::{FromRawFd, RawFd}, os::unix::net::UnixStream, process::ExitCode};

use rsdm_core::{domain::PasswordSecret, ports::{AuthConversation, AuthError, AuthMessage, AuthMessageStyle, CredentialVerifier, VerifyRequest}};
use rsdm_infra::pam::PamCredentialVerifier;

use crate::auth_protocol::{self, Frame};

#[derive(Debug)]
struct HelperConversation {
    stream: UnixStream,
}

impl AuthConversation for HelperConversation {
    fn respond(&mut self, message: AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        let needs_answer = matches!(message.style, AuthMessageStyle::Secret | AuthMessageStyle::Visible);
        auth_protocol::write_frame(&mut self.stream, &Frame::Message(message))
            .map_err(|_| AuthError::InvalidCredentials)?;
        if !needs_answer {
            return Ok(None);
        }
        match auth_protocol::read_frame(&mut self.stream).map_err(|_| AuthError::InvalidCredentials)? {
            Frame::Answer(answer) => Ok(answer.map(PasswordSecret::new)),
            Frame::Cancel => Err(AuthError::InvalidCredentials),
            _ => Err(AuthError::InvalidCredentials),
        }
    }
}

pub(crate) fn run(fd: RawFd) -> ExitCode {
    let result = run_inner(fd);
    if let Err(error) = result {
        tracing::error!(%error, "lock authentication helper failed");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn run_inner(fd: RawFd) -> io::Result<()> {
    // SAFETY: the hidden CLI command receives ownership of this inherited fd.
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
    let Frame::Start { username, service, password } = auth_protocol::read_frame(&mut stream)? else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "authentication helper expected start"));
    };
    let verifier = PamCredentialVerifier;
    let result = verifier.verify(VerifyRequest {
        username: &username,
        password: &PasswordSecret::new(password),
        pam_service: &service,
        conversation: Some(Box::new(HelperConversation { stream: stream.try_clone()? })),
    });
    let (success, message) = match result {
        Ok(()) => (true, String::new()),
        Err(error) => (false, error.to_string()),
    };
    auth_protocol::write_frame(&mut stream, &Frame::Finished { success, message })
}

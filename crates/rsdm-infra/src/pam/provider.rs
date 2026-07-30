use rsdm_core::ports::{
    AuthError, AuthOutcome, AuthProvider, AuthRequest, AuthenticatedSession, CredentialVerifier,
    PamSession, VerifyRequest,
};
use zeroize::Zeroizing;

use super::ffi::PamHandle;

/// NUL-terminated copy of the secret for the C PAM conversation, rejecting
/// embedded NULs which would truncate the password.
fn password_bytes(
    secret: &rsdm_core::domain::PasswordSecret,
) -> Result<Zeroizing<Vec<u8>>, AuthError> {
    let mut password = Zeroizing::new(secret.expose_secret().as_bytes().to_vec());
    if password.contains(&0) {
        return Err(AuthError::Backend("password contains NUL byte".to_string()));
    }
    password.push(0);
    Ok(password)
}

#[derive(Debug, Default)]
pub struct PamAuthProvider;

/// Verifies a password through PAM without opening a session, for the locker.
#[derive(Debug, Default)]
pub struct PamCredentialVerifier;

impl CredentialVerifier for PamCredentialVerifier {
    fn verify(&self, request: VerifyRequest<'_>) -> Result<(), AuthError> {
        tracing::debug!(
            username = request.username,
            pam_service = request.pam_service,
            "verifying credentials with PAM"
        );
        let password = password_bytes(request.password)?;
        let mut handle = PamHandle::start(request.pam_service, request.username, password)?;
        handle.authenticate()?;
        handle.account_mgmt()?;
        tracing::debug!(
            username = request.username,
            "PAM credential verification succeeded"
        );
        // Handle drops here, calling pam_end; no credentials or session opened.
        Ok(())
    }
}

impl AuthProvider for PamAuthProvider {
    fn authenticate(&self, request: AuthRequest<'_>) -> Result<AuthenticatedSession, AuthError> {
        tracing::debug!(
            username = request.username,
            pam_service = request.pam_service,
            tty = request.tty,
            vtnr = ?request.vtnr,
            seat = request.seat,
            "starting PAM authentication"
        );
        let password = password_bytes(request.password)?;
        let mut handle = PamHandle::start(request.pam_service, request.username, password)?;
        handle.authenticate()?;
        tracing::debug!(username = request.username, "PAM authenticate succeeded");
        handle.account_mgmt()?;
        tracing::debug!(
            username = request.username,
            "PAM account management succeeded"
        );
        handle.establish_credentials()?;
        tracing::debug!(username = request.username, "PAM credentials established");
        handle.prepare_session(
            request.tty,
            request.vtnr,
            request.seat,
            request.session_desktop,
        )?;
        tracing::debug!(username = request.username, "PAM environment prepared");

        // Session modules register logind and may unlock the user's keyring.
        if let Err(error) = handle.open_session() {
            let _ = handle.delete_credentials();
            tracing::warn!(username = request.username, %error, "PAM open_session failed");
            return Err(error);
        }
        tracing::debug!(username = request.username, "PAM session opened");

        let environment = handle.environment();
        if let Some(names) = keyring_env_names(&environment) {
            tracing::info!(
                username = request.username,
                published = ?names,
                "keyring agent environment published by PAM session"
            );
        }
        Ok(AuthenticatedSession {
            outcome: AuthOutcome {
                username: request.username.to_string(),
            },
            pam_session: Box::new(OpenedPamSession {
                handle: Some(handle),
                environment,
                session_open: true,
                credentials_established: true,
            }),
        })
    }
}

fn keyring_env_names(environment: &[(String, String)]) -> Option<Vec<&str>> {
    const KEYRING_VARS: &[&str] = &[
        "SSH_AUTH_SOCK",
        "GNOME_KEYRING_CONTROL",
        "GNOME_KEYRING_PID",
        "GPG_AGENT_INFO",
        "KWALLET5_SOCKET",
    ];
    let names: Vec<&str> = environment
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| KEYRING_VARS.contains(name))
        .collect();
    (!names.is_empty()).then_some(names)
}

pub struct OpenedPamSession {
    handle: Option<PamHandle>,
    environment: Vec<(String, String)>,
    session_open: bool,
    credentials_established: bool,
}

impl std::fmt::Debug for OpenedPamSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenedPamSession")
            .field("environment_len", &self.environment.len())
            .field("session_open", &self.session_open)
            .field("credentials_established", &self.credentials_established)
            .finish()
    }
}

impl PamSession for OpenedPamSession {
    fn environment(&self) -> &[(String, String)] {
        &self.environment
    }

    fn close(&mut self) -> Result<(), AuthError> {
        let Some(handle) = self.handle.as_mut() else {
            return Ok(());
        };

        let mut first_error = None;
        if self.session_open {
            if let Err(error) = handle.close_session() {
                first_error = Some(error);
            }
            self.session_open = false;
        }
        if self.credentials_established {
            if let Err(error) = handle.delete_credentials()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
            self.credentials_established = false;
        }
        self.handle.take();

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for OpenedPamSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

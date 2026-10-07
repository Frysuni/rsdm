//! Keep Lock responsive while PAM requests additional credentials.

use std::sync::{Arc, mpsc::TryRecvError};

use rsdm_core::ports::{AuthError, AuthMessageStyle};
use smithay_client_toolkit::seat::keyboard::{KeyEvent, Keysym};

use super::App;
use crate::auth::{Attempt, AuthEvent, start_authentication};

impl App {
    pub(super) fn submit(&mut self) {
        if let Some(job) = self.authentication.as_mut() {
            if job.waiting && !job.cancelled {
                job.waiting = false;
                let _ = job.responses.try_send(Ok(self.model.take_password()));
                self.model.finish_prompt();
            }
            return;
        }
        let result = start_authentication(
            self.model.take_password(), self.ctx.username.clone(), self.ctx.pam_service.clone(),
            Arc::clone(&self.ctx.limiter),
        );
        match result {
            Ok(job) => {
                self.authentication = Some(job);
                self.model.set_info("Authenticating...");
            }
            Err(error) => {
                tracing::error!(%error, "could not start PAM authentication worker");
                self.model.set_error("Authentication is unavailable");
            }
        }
    }

    pub(super) fn process_authentication(&mut self) {
        while let Some(job) = self.authentication.as_mut() {
            let event = match job.events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.finish_authentication(Attempt::Failed("Authentication failed".to_string()));
                    return;
                }
            };
            self.needs_redraw = true;
            match event {
                AuthEvent::Finished(outcome) => {
                    self.finish_authentication(outcome);
                    return;
                }
                AuthEvent::Message(message) if job.cancelled => {
                    if matches!(message.style, AuthMessageStyle::Secret | AuthMessageStyle::Visible) {
                        let _ = job.responses.try_send(Err(AuthError::InvalidCredentials));
                    }
                }
                AuthEvent::Message(message) => match message.style {
                    AuthMessageStyle::Secret | AuthMessageStyle::Visible => {
                        job.waiting = true;
                        self.model.set_prompt(message);
                    }
                    AuthMessageStyle::Info | AuthMessageStyle::Error => self.model.set_auth_notice(message),
                },
            }
        }
    }

    fn finish_authentication(&mut self, outcome: Attempt) {
        let cancelled = self.authentication.take().is_some_and(|job| job.cancelled);
        self.model.finish_prompt();
        self.needs_redraw = true;
        if cancelled {
            self.model.set_error("Authentication cancelled");
            return;
        }
        match outcome {
            Attempt::Unlocked => self.unlock(),
            Attempt::Failed(message) | Attempt::RateLimited(message) => self.model.set_error(message),
        }
    }

    pub(super) fn handle_authentication_key(&mut self, event: KeyEvent) {
        let Some(job) = self.authentication.as_mut() else {
            return;
        };
        if event.keysym == Keysym::Escape {
            job.cancelled = true;
            let _ = job.responses.try_send(Err(AuthError::InvalidCredentials));
            self.model.finish_prompt();
            self.model.set_info("Cancelling authentication...");
        } else if job.waiting && !job.cancelled {
            match event.keysym {
                Keysym::Return | Keysym::KP_Enter => self.submit(),
                Keysym::BackSpace => self.model.backspace(),
                _ => {
                    if let Some(text) = event.utf8 {
                        for ch in text.chars() {
                            self.model.push(ch);
                        }
                    }
                }
            }
        }
        self.needs_redraw = true;
    }
}

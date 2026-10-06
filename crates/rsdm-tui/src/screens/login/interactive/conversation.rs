//! Additional PAM credentials requested while the Greeter retains the VT.

use std::{fs::File, sync::atomic::Ordering, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{Terminal, backend::CrosstermBackend};
use rsdm_core::{
    domain::{PasswordRendering, PasswordSecret},
    ports::{
        AuthConversation, AuthError, AuthMessage, AuthMessageStyle, LoginUiModel, MAX_PASSWORD_BYTES,
    },
};
use rsdm_ui::{Design, Field, Menu, Surface};
use zeroize::Zeroizing;

use super::{Status, build_scene, state::FormState};
use crate::surface::TuiSurface;

pub(super) struct GreeterConversation<'a> {
    pub terminal: &'a mut Terminal<CrosstermBackend<File>>,
    pub model: &'a LoginUiModel<'a>,
    pub form: &'a FormState,
    pub design: &'a Design,
    pub notice: String,
}

impl std::fmt::Debug for GreeterConversation<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GreeterConversation")
    }
}

impl AuthConversation for GreeterConversation<'_> {
    fn respond(&mut self, mut message: AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        if matches!(message.style, AuthMessageStyle::Info | AuthMessageStyle::Error) {
            if self.notice.len() + message.text.len() + 1 > MAX_PASSWORD_BYTES {
                self.notice.clear();
            }
            self.notice.push_str(&message.text);
            self.notice.push('\n');
            message.text = self.notice.clone();
            self.draw(&message, "")?;
            return Ok(None);
        }
        message.text = format!("{}{}", std::mem::take(&mut self.notice), message.text);
        self.read_answer(&message)
    }
}

impl GreeterConversation<'_> {
    fn read_answer(&mut self, message: &AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        let mut input = Zeroizing::new(String::new());
        loop {
            if self.model.terminate.load(Ordering::SeqCst) {
                return Err(AuthError::InvalidCredentials);
            }
            self.draw(message, &input)?;
            match event::poll(Duration::from_millis(200)) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(terminal_error(error)),
            }
            let Event::Key(key) = event::read().map_err(terminal_error)? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.code == KeyCode::Esc
                || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'))
            {
                return Err(AuthError::InvalidCredentials);
            }
            match key.code {
                KeyCode::Enter => return Ok(Some(PasswordSecret::new(std::mem::take(&mut *input)))),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(ch) if !ch.is_control() && input.len() + ch.len_utf8() <= MAX_PASSWORD_BYTES => {
                    input.push(ch);
                }
                _ => {}
            }
        }
    }

    fn draw(&mut self, message: &AuthMessage, input: &str) -> Result<(), AuthError> {
        let status = Status {
            text: message.text.clone(),
            is_error: message.style == AuthMessageStyle::Error,
        };
        let mut scene = build_scene(self.model, self.form, self.design, Some(&status));
        scene.password_preview = if message.style == AuthMessageStyle::Visible {
            input.to_string()
        } else if self.design.password_mode == PasswordRendering::Asterisks {
            "*".repeat(input.chars().count())
        } else {
            String::new()
        };
        scene.field = Field::Password;
        scene.title.clear();
        scene.authentication_active = true;
        scene.pending = None;
        scene.console_exit_enabled = false;
        scene.session_field_visible = false;
        scene.session_picker_enabled = false;
        scene.picker_open = false;
        self.terminal.draw(|frame| {
            let mut surface = TuiSurface::new(frame.buffer_mut());
            surface.clear(self.design.palette().bg_base);
            rsdm_ui::render_login(&mut surface, self.design, &scene, &Menu::new(), 0);
        }).map_err(terminal_error)?;
        Ok(())
    }
}

fn terminal_error(error: std::io::Error) -> AuthError {
    AuthError::Backend(format!("authentication prompt: {error}"))
}

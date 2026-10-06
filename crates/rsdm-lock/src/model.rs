//! Locker UI state. Holds the typed password (zeroized on drop) and the status
//! message; it knows nothing about Wayland, PAM or rendering. The seated
//! username and the look come from elsewhere.

use rsdm_core::{
    domain::{PasswordRendering, PasswordSecret},
    ports::{AuthMessage, AuthMessageStyle, MAX_PASSWORD_BYTES},
};
use zeroize::{Zeroize, Zeroizing};

pub struct LockModel {
    password: Zeroizing<String>,
    message: Option<String>,
    message_is_error: bool,
    password_mode: PasswordRendering,
    prompt: Option<AuthMessage>,
    notice: String,
}

impl LockModel {
    pub fn new(password_mode: PasswordRendering) -> Self {
        Self {
            password: Zeroizing::new(String::new()),
            message: None,
            message_is_error: false,
            password_mode,
            prompt: None,
            notice: String::new(),
        }
    }

    pub fn push(&mut self, ch: char) {
        if !ch.is_control() && self.password.len() + ch.len_utf8() <= MAX_PASSWORD_BYTES {
            self.password.push(ch);
            self.message = None;
        }
    }

    pub fn backspace(&mut self) {
        self.password.pop();
    }

    pub fn clear_password(&mut self) {
        self.password.zeroize();
    }

    pub fn is_empty(&self) -> bool {
        self.password.is_empty()
    }

    /// Move the typed password out for verification, leaving the buffer empty.
    pub fn take_password(&mut self) -> PasswordSecret {
        PasswordSecret::new(std::mem::take(&mut *self.password))
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
        self.message_is_error = true;
    }

    pub fn set_info(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
        self.message_is_error = false;
    }

    pub fn set_auth_notice(&mut self, message: AuthMessage) {
        if self.notice.len() + message.text.len() + 1 > MAX_PASSWORD_BYTES {
            self.notice.clear();
        }
        self.notice.push_str(&message.text);
        self.notice.push('\n');
        self.message = Some(self.notice.clone());
        self.message_is_error = message.style == AuthMessageStyle::Error;
    }

    pub fn set_prompt(&mut self, mut message: AuthMessage) {
        self.clear_password();
        message.text = format!("{}{}", std::mem::take(&mut self.notice), message.text);
        self.prompt = Some(message);
        self.message = None;
        self.message_is_error = false;
    }

    pub fn finish_prompt(&mut self) {
        self.clear_password();
        self.prompt = None;
        self.notice.clear();
        self.set_info("Authenticating...");
    }

    /// The password as shown in the prompt field, masked per the password mode.
    pub fn password_preview(&self) -> String {
        if self.prompt.as_ref().is_some_and(|prompt| prompt.style == AuthMessageStyle::Visible) {
            return self.password.to_string();
        }
        match self.password_mode {
            PasswordRendering::Hidden => String::new(),
            PasswordRendering::Asterisks => "*".repeat(self.password.chars().count()),
        }
    }

    pub fn message(&self) -> Option<&str> {
        self.prompt.as_ref().map(|prompt| prompt.text.as_str()).or(self.message.as_deref())
    }

    pub fn message_is_error(&self) -> bool {
        self.message_is_error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masking_follows_the_mode() {
        let mut model = LockModel::new(PasswordRendering::Asterisks);
        model.push('a');
        model.push('b');
        assert_eq!(model.password_preview(), "**");

        let mut hidden = LockModel::new(PasswordRendering::Hidden);
        hidden.push('a');
        assert_eq!(hidden.password_preview(), "");
    }

    #[test]
    fn take_password_empties_the_buffer() {
        let mut model = LockModel::new(PasswordRendering::Asterisks);
        model.push('s');
        model.push('e');
        let secret = model.take_password();
        assert_eq!(secret.expose_secret(), "se");
        assert!(model.is_empty());
    }

    #[test]
    fn control_characters_are_ignored() {
        let mut model = LockModel::new(PasswordRendering::Asterisks);
        model.push('\n');
        model.push('\t');
        assert!(model.is_empty());
    }

    #[test]
    fn challenge_keeps_instructions_and_masks_only_hidden_input() {
        let mut model = LockModel::new(PasswordRendering::Asterisks);
        model.set_auth_notice(AuthMessage { style: AuthMessageStyle::Info, text: "Use your token".into() });
        model.set_prompt(AuthMessage { style: AuthMessageStyle::Secret, text: "OTP:".into() });
        model.push('1');
        assert_eq!(model.message(), Some("Use your token\nOTP:"));
        assert_eq!(model.password_preview(), "*");
        model.finish_prompt();
        model.set_prompt(AuthMessage { style: AuthMessageStyle::Visible, text: "Account:".into() });
        model.push('a');
        assert_eq!(model.password_preview(), "a");
    }
}

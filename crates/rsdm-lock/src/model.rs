//! Locker UI state. Holds the typed password (zeroized on drop) and the status
//! message; it knows nothing about Wayland, PAM or rendering. The seated
//! username and the look come from elsewhere.

use rsdm_core::domain::{PasswordRendering, PasswordSecret};
use zeroize::Zeroizing;

pub struct LockModel {
    password: Zeroizing<String>,
    message: Option<String>,
    message_is_error: bool,
    password_mode: PasswordRendering,
}

impl LockModel {
    pub fn new(password_mode: PasswordRendering) -> Self {
        Self {
            password: Zeroizing::new(String::new()),
            message: None,
            message_is_error: false,
            password_mode,
        }
    }

    pub fn push(&mut self, ch: char) {
        if !ch.is_control() {
            self.password.push(ch);
            self.message = None;
        }
    }

    pub fn backspace(&mut self) {
        self.password.pop();
    }

    pub fn clear_password(&mut self) {
        self.password.clear();
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

    /// The password as shown in the prompt field, masked per the password mode.
    pub fn password_preview(&self) -> String {
        match self.password_mode {
            PasswordRendering::Hidden => String::new(),
            PasswordRendering::Asterisks => "*".repeat(self.password.chars().count()),
        }
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
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
}

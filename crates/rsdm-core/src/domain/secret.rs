use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PasswordSecret {
    value: String,
}

impl PasswordSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    pub fn expose_secret(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for PasswordSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PasswordSecret([redacted])")
    }
}

impl From<String> for PasswordSecret {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

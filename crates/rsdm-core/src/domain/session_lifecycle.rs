use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    #[default]
    Auto,
    Managed,
    External,
}

impl FromStr for SessionMode {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "managed" => Ok(Self::Managed),
            "external" => Ok(Self::External),
            _ => Err("expected auto, managed or external"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutAction {
    #[default]
    Force,
    Cancel,
}

impl FromStr for TimeoutAction {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "force" => Ok(Self::Force),
            "cancel" => Ok(Self::Cancel),
            _ => Err("expected force or cancel"),
        }
    }
}

impl fmt::Display for TimeoutAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Force => "force",
            Self::Cancel => "cancel",
        })
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownMethod {
    #[default]
    Auto,
    Term,
    Xsmp,
}

impl FromStr for ShutdownMethod {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "term" => Ok(Self::Term),
            "xsmp" => Ok(Self::Xsmp),
            _ => Err("expected auto, term or xsmp"),
        }
    }
}

impl fmt::Display for ShutdownMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::Term => "term",
            Self::Xsmp => "xsmp",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownPolicy {
    pub timeout_secs: u64,
    pub on_timeout: TimeoutAction,
    pub method: ShutdownMethod,
    pub quit_command: Vec<String>,
}

impl Default for ShutdownPolicy {
    fn default() -> Self {
        Self {
            timeout_secs: 30,
            on_timeout: TimeoutAction::Force,
            method: ShutdownMethod::Auto,
            quit_command: Vec::new(),
        }
    }
}

impl ShutdownPolicy {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.timeout_secs == 0 || self.timeout_secs.checked_mul(1_000_000).is_none() {
            return Err("shutdown timeout must be a positive number of seconds representable by systemd");
        }
        if self.quit_command.first().is_some_and(String::is_empty) {
            return Err("quit command must have a nonempty executable");
        }
        if !self.quit_command.is_empty() && self.method != ShutdownMethod::Auto {
            return Err("quit command requires the auto shutdown method");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Starting,
    Running,
    Preparing,
    StoppingSession,
    Closed,
}

impl fmt::Display for SessionPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Preparing => "preparing",
            Self::StoppingSession => "stopping_session",
            Self::Closed => "closed",
        };
        formatter.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_must_fit_the_manager_without_overflow() {
        for timeout_secs in [0, u64::MAX] {
            let policy = ShutdownPolicy { timeout_secs, ..ShutdownPolicy::default() };
            assert!(policy.validate().is_err());
        }
        assert!(ShutdownPolicy::default().validate().is_ok());
    }

    #[test]
    fn quit_argv_preserves_empty_arguments_but_requires_a_program() {
        let mut policy = ShutdownPolicy::default();
        policy.quit_command = vec!["program".into(), "".into()];
        assert!(policy.validate().is_ok());
        policy.quit_command[0].clear();
        assert!(policy.validate().is_err());
    }

    #[test]
    fn explicit_protocol_and_quit_command_cannot_compete() {
        let policy = ShutdownPolicy {
            method: ShutdownMethod::Xsmp,
            quit_command: vec!["program".into()],
            ..ShutdownPolicy::default()
        };
        assert!(policy.validate().is_err());
    }
}

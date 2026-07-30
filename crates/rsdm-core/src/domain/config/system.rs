use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TtyConfig {
    pub path: String,
    /// logind seat the greeter and launched session belong to.
    pub seat: String,
}

impl Default for TtyConfig {
    fn default() -> Self {
        Self {
            path: "/dev/tty1".to_string(),
            seat: "seat0".to_string(),
        }
    }
}

impl TtyConfig {
    /// Virtual terminal number derived from the trailing digits of the device
    /// path (`/dev/tty1` -> `1`). Returns `None` when the path carries no VT
    /// number, e.g. a pseudo terminal used while testing.
    pub fn vtnr(&self) -> Option<u32> {
        vtnr_from_path(&self.path)
    }
}

/// Parse the virtual terminal number from a `/dev/ttyN` style path.
///
/// Only bare virtual terminals (`tty1`, `tty12`) carry a VT number; serial
/// (`ttyS0`), USB (`ttyUSB0`) and pseudo terminals (`pts/3`) deliberately return
/// `None`.
pub fn vtnr_from_path(path: &str) -> Option<u32> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let digits = name.strip_prefix("tty")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathsConfig {
    pub cache_dir: String,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            cache_dir: "/var/cache/rsdm".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecurityConfig {
    pub deny_root: bool,
    pub allowed_groups: Vec<String>,
    pub max_failed_attempts: u8,
    pub failure_delay_ms: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            deny_root: true,
            allowed_groups: Vec::new(),
            max_failed_attempts: 5,
            failure_delay_ms: 1200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FallbackConfig {
    /// When the greeter cannot run (TTY unavailable, no sessions, PAM misconfig)
    /// hand the terminal to a console login so the operator can still log in.
    pub enabled: bool,
    /// Argv of the fallback program. The first element is resolved through
    /// `PATH` when it contains no `/`. Leave empty to auto-resolve: rsdm reuses
    /// the console login systemd already runs on the VT (`getty@<tty>.service`,
    /// usually `agetty`) and falls back to built-in `agetty`/`login` candidates,
    /// so no command needs configuring on a typical system.
    pub command: Vec<String>,
}

impl Default for FallbackConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            // Empty == auto: discover the system getty, then try the built-ins.
            command: Vec::new(),
        }
    }
}

/// Logging. The sink is always journald (rsdm logs to stderr, the systemd unit
/// routes it to the journal), which is not configurable. `file`, when set, adds
/// an extra plain-text log file alongside the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: LoggingLevel,
    pub file: Option<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LoggingLevel::Info,
            file: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoggingLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LoggingLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vtnr_from_device_path() {
        assert_eq!(vtnr_from_path("/dev/tty1"), Some(1));
        assert_eq!(vtnr_from_path("/dev/tty12"), Some(12));
        assert_eq!(TtyConfig::default().vtnr(), Some(1));
    }

    #[test]
    fn missing_vtnr_is_none() {
        assert_eq!(vtnr_from_path("/dev/pts/3"), None);
        assert_eq!(vtnr_from_path("/dev/console"), None);
    }
}

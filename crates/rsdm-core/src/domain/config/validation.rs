use std::path::Path;

use thiserror::Error;

use super::{AppConfig, MAX_LOCK_SIZE};

impl AppConfig {
    pub fn validation_errors(&self) -> Vec<ConfigIssue> {
        let mut issues = Vec::new();

        validate_dm(self, &mut issues);
        validate_session_manager(self, &mut issues);
        validate_security_and_paths(self, &mut issues);
        validate_lock(self, &mut issues);
        validate_idle(self, &mut issues);
        issues.extend(design_issues("dm.design", &self.dm.design));
        issues.extend(design_issues("lock.design", &self.lock.design));

        issues
    }

    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        let issues = self.validation_errors();
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ConfigValidationError { issues })
        }
    }

    pub fn validation_warnings(&self) -> Vec<ConfigIssue> {
        let mut warnings = Vec::new();
        if self.dm.tty.path == "/dev/tty1" {
            // TTYReset/TTYVHangup/TTYVTDisallocate must NOT be recommended: systemd
            // applies them after the unit stops too, onto a VT that by then belongs
            // to the user's live compositor session.
            warnings.push(ConfigIssue::new(
                "dm.tty.path",
                "cannot verify systemd service wiring; ensure StandardOutput/StandardError=journal and TTYPath=/dev/tty1, and do not set TTYReset/TTYVHangup/TTYVTDisallocate",
            ));
        }
        warnings
    }
}

fn validate_dm(config: &AppConfig, issues: &mut Vec<ConfigIssue>) {
    if config.dm.tty.vtnr().is_none() {
        issues.push(ConfigIssue::new(
            "dm.tty.path",
            "must point at a Linux virtual terminal such as /dev/tty1",
        ));
    }
    if config.dm.tty.seat.trim().is_empty() {
        issues.push(ConfigIssue::new("dm.tty.seat", "must not be empty"));
    }
    if config.dm.pam_service.trim().is_empty() {
        issues.push(ConfigIssue::new("dm.pam_service", "must not be empty"));
    }
    if config
        .dm
        .fixed_session
        .as_deref()
        .is_some_and(|session| session.trim().is_empty())
    {
        issues.push(ConfigIssue::new(
            "dm.fixed_session",
            "must be a session id or command, or omitted for the picker",
        ));
    }
    if config.dm.uses_picker() && config.dm.session_dirs.is_empty() {
        issues.push(ConfigIssue::new(
            "dm.session_dirs",
            "must contain at least one wayland-sessions directory",
        ));
    }
    if config
        .dm
        .session_dirs
        .iter()
        .any(|path| path.trim().is_empty() || !Path::new(path).is_absolute())
    {
        issues.push(ConfigIssue::new(
            "dm.session_dirs",
            "must contain absolute wayland-sessions directories",
        ));
    }
    if has_empty_argument(&config.dm.fallback.command) {
        issues.push(ConfigIssue::new(
            "dm.fallback.command",
            "must be empty for automatic discovery or contain only non-empty argv entries",
        ));
    }
}

fn validate_session_manager(config: &AppConfig, issues: &mut Vec<ConfigIssue>) {
    if config
        .session_manager
        .extra_env
        .iter()
        .any(|name| !valid_environment_name(name))
    {
        issues.push(ConfigIssue::new(
            "session_manager.extra_env",
            "must contain valid environment variable names",
        ));
    }
}

fn validate_security_and_paths(config: &AppConfig, issues: &mut Vec<ConfigIssue>) {
    if config.security.max_failed_attempts == 0 {
        issues.push(ConfigIssue::new(
            "security.max_failed_attempts",
            "must be at least 1",
        ));
    }
    if config
        .security
        .allowed_groups
        .iter()
        .any(|group| group.trim().is_empty())
    {
        issues.push(ConfigIssue::new(
            "security.allowed_groups",
            "must contain only non-empty group names",
        ));
    }
    if config.paths.cache_dir.trim().is_empty() {
        issues.push(ConfigIssue::new("paths.cache_dir", "must not be empty"));
    } else if !Path::new(&config.paths.cache_dir).is_absolute() {
        issues.push(ConfigIssue::new(
            "paths.cache_dir",
            "must be an absolute path",
        ));
    }
    if config
        .logging
        .file
        .as_deref()
        .is_some_and(|file| file.trim().is_empty() || !Path::new(file).is_absolute())
    {
        issues.push(ConfigIssue::new(
            "logging.file",
            "must be an absolute log file path",
        ));
    }
}

fn validate_lock(config: &AppConfig, issues: &mut Vec<ConfigIssue>) {
    if config.lock.pam_service.trim().is_empty() {
        issues.push(ConfigIssue::new("lock.pam_service", "must not be empty"));
    }
    if config
        .lock
        .primary_output
        .as_deref()
        .is_some_and(|output| output.trim().is_empty())
    {
        issues.push(ConfigIssue::new(
            "lock.primary_output",
            "must be a Wayland output name, or omitted for automatic selection",
        ));
    }
    if config
        .lock
        .size
        .is_some_and(|size| !(1..=MAX_LOCK_SIZE).contains(&size))
    {
        issues.push(ConfigIssue::new(
            "lock.size",
            "must be between 1 and 12, or omitted for TTY-like automatic sizing",
        ));
    }
}

fn validate_idle(config: &AppConfig, issues: &mut Vec<ConfigIssue>) {
    if config.idle.timeout == 0 || config.idle.timeout > u64::from(u32::MAX / 1000) {
        issues.push(ConfigIssue::new(
            "idle.timeout",
            "must be between 1 and 4294967 seconds",
        ));
    }
    if has_empty_argument(&config.idle.lock_command) {
        issues.push(ConfigIssue::new(
            "idle.lock_command",
            "must be empty for the built-in locker or contain only non-empty argv entries",
        ));
    }
    if config.idle.enable && config.idle.lock_command.is_empty() && !config.lock.enable {
        issues.push(ConfigIssue::new(
            "idle.enable",
            "requires lock.enable = true when idle.lock_command uses the built-in locker",
        ));
    }
    if config
        .idle
        .on_lock
        .iter()
        .chain(&config.idle.on_unlock)
        .any(|command| command.trim().is_empty())
    {
        issues.push(ConfigIssue::new(
            "idle.on_lock/on_unlock",
            "must not contain empty shell commands",
        ));
    }
}

fn has_empty_argument(command: &[String]) -> bool {
    command.iter().any(|part| part.trim().is_empty())
}

fn design_issues(prefix: &'static str, design: &super::DesignConfig) -> Vec<ConfigIssue> {
    let mut issues = Vec::new();
    if design.background_speed > 10 {
        issues.push(ConfigIssue::new(
            if prefix == "dm.design" {
                "dm.design.background_speed"
            } else {
                "lock.design.background_speed"
            },
            "must be between 0 and 10",
        ));
    }
    if design.wallpaper_dim > 10 {
        issues.push(ConfigIssue::new(
            if prefix == "dm.design" {
                "dm.design.wallpaper_dim"
            } else {
                "lock.design.wallpaper_dim"
            },
            "must be between 0 and 10",
        ));
    }
    if design.background_opacity > 10 {
        issues.push(ConfigIssue::new(
            if prefix == "dm.design" {
                "dm.design.background_opacity"
            } else {
                "lock.design.background_opacity"
            },
            "must be between 0 and 10",
        ));
    }
    if let Some(wallpaper) = design.wallpaper.as_deref()
        && (wallpaper.trim().is_empty() || !Path::new(wallpaper).is_absolute())
    {
        issues.push(ConfigIssue::new(
            if prefix == "dm.design" {
                "dm.design.wallpaper"
            } else {
                "lock.design.wallpaper"
            },
            "must be an absolute path to an image",
        ));
    }
    issues
}

fn valid_environment_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    pub field: &'static str,
    pub message: &'static str,
}

impl ConfigIssue {
    pub const fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("configuration has {count} issue(s)", count = .issues.len())]
pub struct ConfigValidationError {
    pub issues: Vec<ConfigIssue>,
}

#[cfg(test)]
mod tests {
    use crate::domain::AppConfig;

    #[test]
    fn default_config_is_valid() {
        assert!(AppConfig::default().validate().is_ok());
    }

    #[test]
    fn empty_fixed_session_is_rejected() {
        let mut config = AppConfig::default();
        config.dm.fixed_session = Some(String::new());
        let issues = config.validation_errors();
        assert!(issues.iter().any(|issue| issue.field == "dm.fixed_session"));
    }

    #[test]
    fn tty_path_must_be_a_virtual_terminal() {
        let mut config = AppConfig::default();
        config.dm.tty.path = "/dev/pts/0".to_string();
        let issues = config.validation_errors();
        assert!(issues.iter().any(|issue| issue.field == "dm.tty.path"));

        config.dm.tty.path = "/dev/ttyS0".to_string();
        assert!(
            config
                .validation_errors()
                .iter()
                .any(|issue| issue.field == "dm.tty.path")
        );
    }

    #[test]
    fn relative_cache_dir_is_rejected() {
        let mut config = AppConfig::default();
        config.paths.cache_dir = "relative/cache".to_string();
        let issues = config.validation_errors();
        assert!(issues.iter().any(|issue| issue.field == "paths.cache_dir"));
    }

    #[test]
    fn relative_lock_wallpaper_is_rejected() {
        let mut config = AppConfig::default();
        config.lock.design.wallpaper = Some("wallpaper.png".to_string());
        let issues = config.validation_errors();
        assert!(
            issues
                .iter()
                .any(|issue| issue.field == "lock.design.wallpaper")
        );
    }

    #[test]
    fn design_levels_stay_in_documented_range() {
        let mut config = AppConfig::default();
        config.dm.design.background_speed = 11;
        config.lock.design.wallpaper_dim = 11;
        config.lock.design.background_opacity = u8::MAX;
        let issues = config.validation_errors();
        assert!(
            issues
                .iter()
                .any(|issue| issue.field == "dm.design.background_speed")
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.field == "lock.design.wallpaper_dim")
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.field == "lock.design.background_opacity")
        );
    }

    #[test]
    fn empty_argv_and_identity_entries_are_rejected() {
        let mut config = AppConfig::default();
        config.dm.tty.seat.clear();
        config.dm.fallback.command = vec!["agetty".to_string(), " ".to_string()];
        config.security.allowed_groups = vec![String::new()];
        let issues = config.validation_errors();
        assert!(issues.iter().any(|issue| issue.field == "dm.tty.seat"));
        assert!(
            issues
                .iter()
                .any(|issue| issue.field == "dm.fallback.command")
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.field == "security.allowed_groups")
        );
    }

    #[test]
    fn session_manager_extra_env_must_be_environment_names() {
        let mut config = AppConfig::default();
        config.session_manager.extra_env = vec!["VALID_NAME".to_string(), "bad-name".to_string()];
        let issues = config.validation_errors();
        assert!(
            issues
                .iter()
                .any(|issue| issue.field == "session_manager.extra_env")
        );
    }

    #[test]
    fn file_logging_requires_an_absolute_path() {
        let mut config = AppConfig::default();
        config.logging.file = Some("rsdm.log".to_string());
        let issues = config.validation_errors();
        assert!(issues.iter().any(|issue| issue.field == "logging.file"));
    }

    #[test]
    fn empty_primary_lock_output_is_rejected() {
        let mut config = AppConfig::default();
        config.lock.primary_output = Some("  ".to_string());
        let issues = config.validation_errors();
        assert!(
            issues
                .iter()
                .any(|issue| issue.field == "lock.primary_output")
        );
    }

    #[test]
    fn idle_timeout_must_fit_the_wayland_protocol() {
        let mut config = AppConfig::default();
        config.idle.timeout = 0;
        assert!(
            config
                .validation_errors()
                .iter()
                .any(|issue| issue.field == "idle.timeout")
        );

        config.idle.timeout = u64::from(u32::MAX / 1000) + 1;
        assert!(
            config
                .validation_errors()
                .iter()
                .any(|issue| issue.field == "idle.timeout")
        );
    }

    #[test]
    fn idle_commands_must_not_have_empty_entries() {
        let mut config = AppConfig::default();
        config.idle.lock_command = vec!["rsdm".to_string(), " ".to_string()];
        config.idle.on_lock = vec![String::new()];
        let issues = config.validation_errors();
        assert!(
            issues
                .iter()
                .any(|issue| issue.field == "idle.lock_command")
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.field == "idle.on_lock/on_unlock")
        );
    }

    #[test]
    fn built_in_idle_requires_the_locker() {
        let mut config = AppConfig::default();
        config.idle.enable = true;
        let issues = config.validation_errors();
        assert!(issues.iter().any(|issue| issue.field == "idle.enable"));

        config.idle.lock_command = vec!["swaylock".to_string()];
        assert!(config.validate().is_ok());
    }
}

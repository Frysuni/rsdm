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
    config.idle.on_lock = vec![Vec::new()];
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

#[test]
fn readiness_timeout_must_fit_a_monotonic_deadline() {
    let mut config = AppConfig::default();
    for timeout in [i64::MAX as u64, u64::MAX] {
        config.session_manager.ready_timeout_secs = timeout;
        let issues = config.validation_errors();
        assert!(
            issues.iter().any(|issue| issue.field == "session_manager.ready_timeout_secs")
        );
    }

    config.session_manager.enabled = false;
    assert!(config.validate().is_err());
}

#[test]
fn zero_and_ordinary_readiness_timeouts_are_valid() {
    let mut config = AppConfig::default();
    for timeout in [0, 10, 3600] {
        config.session_manager.ready_timeout_secs = timeout;
        assert!(config.validate().is_ok());
    }
}

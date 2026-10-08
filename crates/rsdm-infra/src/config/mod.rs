use std::{
    env,
    fs,
    path::{Path, PathBuf},
};

use rsdm_core::domain::{AppConfig, ConfigValidationError, DesignConfig, SecondaryOutput};
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod save;

pub use save::{ConfigSaveError, save_lock_runtime_settings};

pub const DEFAULT_CONFIG_PATH: &str = "/etc/rsdm.toml";

pub fn load_config(path: &Path) -> Result<AppConfig, ConfigLoadError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigLoadError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let mut config = parse_config(&text, Some(path))?;
    apply_lock_runtime_overlay(&mut config)?;
    config
        .validate()
        .map_err(|source| ConfigLoadError::Validate {
            path: display_path(Some(path)),
            source,
        })?;
    Ok(config)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LockRuntimeFile {
    pub(crate) lock: LockRuntimeSettings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LockRuntimeSettings {
    pub(crate) size: Option<u8>,
    pub(crate) size_reset: bool,
    pub(crate) secondary_output: Option<SecondaryOutput>,
    pub(crate) design: LockRuntimeDesign,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LockRuntimeDesign {
    pub(crate) theme: Option<rsdm_core::domain::ThemePreset>,
    pub(crate) border_style: Option<rsdm_core::domain::BorderStyle>,
    pub(crate) background: Option<rsdm_core::domain::Background>,
    pub(crate) background_speed: Option<u8>,
    pub(crate) title_font: Option<String>,
    pub(crate) wallpaper_dim: Option<u8>,
    pub(crate) background_opacity: Option<u8>,
}

pub(crate) fn lock_runtime_path() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".config"))
                .filter(|path| path.is_absolute())
        })
        .map(|base| base.join("rsdm/lock.toml"))
}

fn apply_lock_runtime_overlay(config: &mut AppConfig) -> Result<(), ConfigLoadError> {
    let Some(path) = lock_runtime_path() else {
        return Ok(());
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(ConfigLoadError::Io {
                path: path.display().to_string(),
                source,
            });
        }
    };
    let overlay =
        toml::from_str::<LockRuntimeFile>(&text).map_err(|source| ConfigLoadError::Parse {
            path: path.display().to_string(),
            source,
        })?;
    apply_lock_runtime_settings(config, overlay);
    Ok(())
}

fn apply_lock_runtime_settings(config: &mut AppConfig, overlay: LockRuntimeFile) {
    let lock = overlay.lock;
    if lock.size_reset {
        config.lock.size = None;
    } else if let Some(size) = lock.size {
        config.lock.size = Some(size);
    }
    if let Some(output) = lock.secondary_output {
        config.lock.secondary_output = output;
    }
    apply_design_overlay(&mut config.lock.design, lock.design);
}

fn apply_design_overlay(design: &mut DesignConfig, overlay: LockRuntimeDesign) {
    if let Some(value) = overlay.theme {
        design.theme = value;
    }
    if let Some(value) = overlay.border_style {
        design.border_style = value;
    }
    if let Some(value) = overlay.background {
        design.background = value;
    }
    if let Some(value) = overlay.background_speed {
        design.background_speed = value;
    }
    if let Some(value) = overlay.title_font {
        design.title_font = value;
    }
    if let Some(value) = overlay.wallpaper_dim {
        design.wallpaper_dim = value;
    }
    if let Some(value) = overlay.background_opacity {
        design.background_opacity = value;
    }
}

pub fn parse_config(input: &str, path: Option<&Path>) -> Result<AppConfig, ConfigLoadError> {
    let config = toml::from_str::<AppConfig>(input).map_err(|source| ConfigLoadError::Parse {
        path: display_path(path),
        source,
    })?;
    config
        .validate()
        .map_err(|source| ConfigLoadError::Validate {
            path: display_path(path),
            source,
        })?;
    Ok(config)
}

fn display_path(path: Option<&Path>) -> String {
    path.map_or_else(|| "<inline>".to_string(), |path| path.display().to_string())
}

#[derive(Debug, Error)]
pub enum ConfigLoadError {
    #[error("failed to read config at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config at {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid config at {path}: {source}")]
    Validate {
        path: String,
        #[source]
        source: ConfigValidationError,
    },
}

#[cfg(test)]
mod tests {
    use rsdm_core::domain::SecondaryOutput;

    use super::*;

    #[test]
    fn parses_lock_output_policy() {
        let config = parse_config(
            r#"
                [lock]
                primary_output = "DP-1"
                secondary_output = "off"
            "#,
            None,
        )
        .expect("valid output policy");

        assert_eq!(config.lock.primary_output.as_deref(), Some("DP-1"));
        assert_eq!(config.lock.secondary_output, SecondaryOutput::Off);
    }

    #[test]
    fn rejects_unknown_lock_output_policy() {
        let result = parse_config(
            r#"
                [lock]
                secondary_output = "mirror"
            "#,
            None,
        );

        assert!(matches!(result, Err(ConfigLoadError::Parse { .. })));
    }

    #[test]
    fn parses_and_validates_lock_size() {
        let config = parse_config("[lock]\nsize = 2\n", None).expect("valid lock size");
        assert_eq!(config.lock.size, Some(2));

        let error = parse_config("[lock]\nsize = 0\n", None).expect_err("invalid lock size");
        assert!(matches!(error, ConfigLoadError::Validate { .. }));
    }

    #[test]
    fn parses_idle_policy_and_hooks() {
        let config = parse_config(
            r#"
                [lock]
                enable = true

                [idle]
                enable = true
                timeout = 90
                ignore_inhibitors = true
                lock_command = []
                on_lock = [["notify-send", "locked"]]
                on_unlock = [["notify-send", "unlocked"]]
            "#,
            None,
        )
        .expect("valid idle policy");

        assert!(config.idle.enable);
        assert_eq!(config.idle.timeout, 90);
        assert!(config.idle.ignore_inhibitors);
        assert_eq!(config.idle.on_lock, [["notify-send", "locked"]]);
        assert_eq!(config.idle.on_unlock, [["notify-send", "unlocked"]]);
    }

    #[test]
    fn rejects_unrepresentable_readiness_timeout_during_config_loading() {
        let error = parse_config(
            "[session_manager]\nready_timeout_secs = 9223372036854775807\n",
            None,
        )
        .expect_err("timeout cannot fit a monotonic deadline");
        let ConfigLoadError::Validate { source, .. } = error else {
            panic!("expected config validation to reject the timeout");
        };
        assert!(
            source.issues.iter().any(|issue| issue.field == "session_manager.ready_timeout_secs")
        );
    }
}

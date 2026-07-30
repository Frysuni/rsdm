use std::{fs, path::Path};

use rsdm_core::domain::{AppConfig, ConfigValidationError};
use thiserror::Error;

mod save;

pub use save::{ConfigSaveError, save_lock_runtime_settings};

pub const DEFAULT_CONFIG_PATH: &str = "/etc/rsdm.toml";

pub fn load_config(path: &Path) -> Result<AppConfig, ConfigLoadError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigLoadError::Io {
        path: path.display().to_string(),
        source,
    })?;
    parse_config(&text, Some(path))
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
                on_lock = ["notify-send locked"]
                on_unlock = ["notify-send unlocked"]
            "#,
            None,
        )
        .expect("valid idle policy");

        assert!(config.idle.enable);
        assert_eq!(config.idle.timeout, 90);
        assert!(config.idle.ignore_inhibitors);
        assert_eq!(config.idle.on_lock, ["notify-send locked"]);
        assert_eq!(config.idle.on_unlock, ["notify-send unlocked"]);
    }
}

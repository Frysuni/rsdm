//! User-owned persistence for visual settings changed by the lock UI.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use rsdm_core::domain::{DesignConfig, SecondaryOutput};
use thiserror::Error;

use super::{LockRuntimeDesign, LockRuntimeFile, LockRuntimeSettings, lock_runtime_path};

/// Persist the visual settings exposed by the lock menu in the user's config
/// directory. The system DM configuration is never rewritten by this path.
pub fn save_lock_runtime_settings(
    _config_path: &Path,
    design: &DesignConfig,
    size: Option<u8>,
    secondary_output: SecondaryOutput,
) -> Result<(), ConfigSaveError> {
    let path = lock_runtime_path().ok_or(ConfigSaveError::MissingHome)?;
    save_to_path(&path, design, size, secondary_output)
}

fn save_to_path(
    path: &Path,
    design: &DesignConfig,
    size: Option<u8>,
    secondary_output: SecondaryOutput,
) -> Result<(), ConfigSaveError> {
    let parent = path.parent().ok_or_else(|| ConfigSaveError::Path {
        path: path.to_path_buf(),
        message: "runtime config path has no parent directory",
    })?;
    fs::create_dir_all(parent).map_err(|source| ConfigSaveError::Directory {
        path: parent.to_path_buf(),
        source,
    })?;
    set_private_directory(parent)?;

    let file = LockRuntimeFile {
        lock: LockRuntimeSettings {
            size,
            secondary_output: Some(secondary_output),
            design: LockRuntimeDesign {
                theme: Some(design.theme),
                border_style: Some(design.border_style),
                background: Some(design.background),
                background_speed: Some(design.background_speed),
                title_font: Some(design.title_font.clone()),
                wallpaper_dim: Some(design.wallpaper_dim),
                background_opacity: Some(design.background_opacity),
            },
        },
    };
    let rendered = toml::to_string_pretty(&file).map_err(ConfigSaveError::Serialize)?;
    crate::atomic_file::write(
        path,
        rendered.as_bytes(),
        crate::atomic_file::AtomicWriteOptions {
            mode: 0o600,
            sync_file: true,
            sync_parent: true,
        },
    )
    .map_err(|source| ConfigSaveError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), ConfigSaveError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        ConfigSaveError::Directory {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), ConfigSaveError> {
    Ok(())
}

#[derive(Debug, Error)]
pub enum ConfigSaveError {
    #[error("cannot determine the user configuration directory")]
    MissingHome,
    #[error("cannot prepare user configuration directory {path}: {source}")]
    Directory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid runtime configuration path {path}: {message}")]
    Path { path: PathBuf, message: &'static str },
    #[error("cannot serialize runtime lock settings: {0}")]
    Serialize(#[source] toml::ser::Error),
    #[error("cannot write runtime lock settings to {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use rsdm_core::domain::{Background, ThemePreset};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rsdm-config-save-{}-{}-{name}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn saves_only_lock_visual_settings_in_user_file() {
        let directory = test_path("runtime");
        let path = directory.join("rsdm/lock.toml");
        let design = DesignConfig {
            theme: ThemePreset::Nord,
            background: Background::Matrix,
            ..DesignConfig::default()
        };
        save_to_path(&path, &design, Some(2), SecondaryOutput::Black).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[lock.design]"));
        assert!(!text.contains("enable"));
        assert!(!text.contains("pam_service"));
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn overlay_does_not_replace_administrative_settings() {
        let mut config = rsdm_core::domain::AppConfig::default();
        config.lock.enable = false;
        config.lock.pam_service = "admin-policy".into();
        let overlay = LockRuntimeFile {
            lock: LockRuntimeSettings {
                size: Some(2),
                secondary_output: Some(SecondaryOutput::Black),
                design: LockRuntimeDesign {
                    theme: Some(ThemePreset::Nord),
                    ..Default::default()
                },
            },
        };
        super::super::apply_lock_runtime_settings(&mut config, overlay);
        assert!(!config.lock.enable);
        assert_eq!(config.lock.pam_service, "admin-policy");
        assert_eq!(config.lock.size, Some(2));
        assert_eq!(config.lock.design.theme, ThemePreset::Nord);
    }
}

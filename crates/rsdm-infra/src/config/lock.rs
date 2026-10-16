//! Personal visual preferences loaded only when starting Lock.

use std::{env, fs, path::{Path, PathBuf}};

use rsdm_core::domain::{AppConfig, DesignConfig, SecondaryOutput};
use serde::{Deserialize, Serialize};

use super::{ConfigLoadError, load_config};

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

/// Add the current user's visual preferences to the validated Lock settings.
pub fn load_lock_config(config_path: &Path) -> Result<AppConfig, ConfigLoadError> {
    let mut config = load_config(config_path)?;
    let Some(path) = lock_runtime_path() else {
        return Ok(config);
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(config),
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
    apply_lock_runtime_settings(&mut config, overlay);
    config.validate().map_err(|source| ConfigLoadError::Validate {
        path: path.display().to_string(),
        source,
    })?;
    Ok(config)
}

pub(super) fn apply_lock_runtime_settings(config: &mut AppConfig, overlay: LockRuntimeFile) {
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

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;

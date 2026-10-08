//! Comment-preserving, atomic persistence for settings changed by the lock UI.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use rsdm_core::domain::{DesignConfig, SecondaryOutput};
use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table, value};

use super::parse_config;

const LIVE_DESIGN_KEYS: &[&str] = &[
    "theme",
    "border_style",
    "background",
    "background_speed",
    "title_font",
    "wallpaper_dim",
    "background_opacity",
];

/// Persist exactly the values that can be changed from the lock screen. The
/// original TOML document, comments, ordering, and unrelated settings survive.
/// The replacement is written and synced in the same directory before rename,
/// so interruption cannot leave a partially written configuration.
pub fn save_lock_runtime_settings(
    config_path: &Path,
    design: &DesignConfig,
    size: Option<u8>,
    secondary_output: SecondaryOutput,
) -> Result<(), ConfigSaveError> {
    let target = resolve_target(config_path)?;
    let original = fs::read_to_string(&target).map_err(|source| ConfigSaveError::Read {
        path: target.clone(),
        source,
    })?;
    let mut document =
        original
            .parse::<DocumentMut>()
            .map_err(|source| ConfigSaveError::Parse {
                path: target.clone(),
                source,
            })?;

    let lock = table_mut(&mut document, "lock", &target)?;
    match size {
        Some(size) => set_preserving_decor(lock, "size", value(i64::from(size))),
        None => {
            lock.remove("size");
        }
    }
    set_preserving_decor(lock, "secondary_output", value(secondary_output.as_str()));

    if !lock.contains_key("design") {
        lock["design"] = Item::Table(Table::new());
    }
    let target_design = lock["design"]
        .as_table_mut()
        .ok_or_else(|| ConfigSaveError::Shape {
            path: target.clone(),
            message: "[lock.design] is not a TOML table",
        })?;
    let serialized_design = toml::to_string(design).map_err(ConfigSaveError::Serialize)?;
    let source_design = serialized_design
        .parse::<DocumentMut>()
        .map_err(|source| ConfigSaveError::ParseGenerated { source })?;
    for key in LIVE_DESIGN_KEYS {
        if let Some(item) = source_design.get(key) {
            set_preserving_decor(target_design, key, item.clone());
        }
    }

    let rendered = document.to_string();
    parse_config(&rendered, Some(&target)).map_err(ConfigSaveError::Validation)?;
    atomic_replace(&target, rendered.as_bytes())
}

/// Replacing a scalar through `Table::insert` resets its trailing comment.
/// Carry the old value decoration over so `foo = old # explanation` becomes
/// `foo = new # explanation` instead of silently deleting documentation.
fn set_preserving_decor(table: &mut Table, key: &str, mut item: Item) {
    if let Some(decor) = table
        .get(key)
        .and_then(Item::as_value)
        .map(|value| value.decor().clone())
        && let Some(value) = item.as_value_mut()
    {
        *value.decor_mut() = decor;
    }
    table.insert(key, item);
}

fn table_mut<'a>(
    document: &'a mut DocumentMut,
    name: &str,
    path: &Path,
) -> Result<&'a mut Table, ConfigSaveError> {
    if !document.contains_key(name) {
        document[name] = Item::Table(Table::new());
    }
    document[name]
        .as_table_mut()
        .ok_or_else(|| ConfigSaveError::Shape {
            path: path.to_path_buf(),
            message: "[lock] is not a TOML table",
        })
}

fn resolve_target(config_path: &Path) -> Result<PathBuf, ConfigSaveError> {
    let target = fs::canonicalize(config_path).map_err(|source| ConfigSaveError::Read {
        path: config_path.to_path_buf(),
        source,
    })?;
    if target.starts_with("/nix/store") {
        return Err(ConfigSaveError::DeclarativeNixOs {
            path: config_path.to_path_buf(),
            target,
        });
    }
    let metadata = fs::metadata(&target).map_err(|source| ConfigSaveError::Read {
        path: target.clone(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(ConfigSaveError::Shape {
            path: target,
            message: "configuration path does not resolve to a regular file",
        });
    }
    Ok(target)
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), ConfigSaveError> {
    let metadata = fs::metadata(path).map_err(|source| ConfigSaveError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    crate::atomic_file::write(
        path,
        bytes,
        crate::atomic_file::AtomicWriteOptions {
            mode: metadata.permissions().mode() & 0o777,
            sync_file: true,
            sync_parent: true,
        },
    )
    .map_err(|source| classify_write_error(path, source))
}

fn classify_write_error(path: &Path, source: std::io::Error) -> ConfigSaveError {
    if source.kind() == std::io::ErrorKind::PermissionDenied {
        ConfigSaveError::PermissionDenied {
            path: path.to_path_buf(),
            source,
        }
    } else if source.raw_os_error() == Some(libc::EROFS) {
        ConfigSaveError::ReadOnlyFilesystem {
            path: path.to_path_buf(),
            source,
        }
    } else {
        ConfigSaveError::Write {
            path: path.to_path_buf(),
            source,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigSaveError {
    #[error("cannot read configuration at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse configuration at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml_edit::TomlError,
    },
    #[error("cannot serialize runtime design: {0}")]
    Serialize(#[source] toml::ser::Error),
    #[error("internal error while preparing runtime settings: {source}")]
    ParseGenerated {
        #[source]
        source: toml_edit::TomlError,
    },
    #[error("cannot update {path}: {message}")]
    Shape {
        path: PathBuf,
        message: &'static str,
    },
    #[error(
        "refusing to modify NixOS-generated {path} (target: {target}); set services.rsdm.lock.size/secondaryOutput/design in configuration.nix and run nixos-rebuild switch"
    )]
    DeclarativeNixOs { path: PathBuf, target: PathBuf },
    #[error(
        "cannot save {path}: permission denied; use a user-writable --config file or edit the system/declarative configuration (do not run the locker as root): {source}"
    )]
    PermissionDenied {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "cannot save {path}: the filesystem is read-only; remount it writable or update the declarative system configuration: {source}"
    )]
    ReadOnlyFilesystem {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot atomically save {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("refusing to write settings because the resulting configuration is invalid: {0}")]
    Validation(#[source] super::ConfigLoadError),
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
    fn updates_only_runtime_fields_and_preserves_comments() {
        let directory = test_path("preserve");
        fs::create_dir_all(&directory).expect("test directory");
        let path = directory.join("config.toml");
        fs::write(
            &path,
            r#"# keep this comment
[lock]
enable = true
secondary_output = "background" # keep inline too

[lock.design]
theme = "monochrome"
title_text = "do not replace"
"#,
        )
        .expect("seed config");

        let design = DesignConfig {
            theme: ThemePreset::Nord,
            background: Background::Matrix,
            ..DesignConfig::default()
        };
        save_lock_runtime_settings(&path, &design, Some(2), SecondaryOutput::Black)
            .expect("save settings");

        let text = fs::read_to_string(&path).expect("saved config");
        assert!(text.contains("# keep this comment"));
        assert!(text.contains("# keep inline too"));
        assert!(text.contains("title_text = \"do not replace\""));
        let config = super::super::parse_config(&text, Some(&path)).expect("valid config");
        assert_eq!(config.lock.size, Some(2));
        assert_eq!(config.lock.secondary_output, SecondaryOutput::Black);
        assert_eq!(config.lock.design.theme, ThemePreset::Nord);
        assert_eq!(config.lock.design.background, Background::Matrix);

        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn saving_auto_removes_an_explicit_size() {
        let directory = test_path("auto");
        fs::create_dir_all(&directory).expect("test directory");
        let path = directory.join("config.toml");
        fs::write(&path, "[lock]\nsize = 7\n").expect("seed config");

        save_lock_runtime_settings(
            &path,
            &DesignConfig::default(),
            None,
            SecondaryOutput::Background,
        )
        .expect("save settings");
        let config = super::super::load_config(&path).expect("valid config");
        assert_eq!(config.lock.size, None);

        fs::remove_dir_all(directory).expect("cleanup");
    }
}

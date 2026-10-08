use std::{
    fs,
    path::{Path, PathBuf},
};

use rsdm_core::ports::{StorageError, UserStore};
use serde::{Deserialize, Serialize};

mod state_file;

const REMEMBERED_FILE: &str = "remembered.toml";
#[derive(Debug, Clone)]
pub struct FileUserStore {
    path: PathBuf,
}

impl FileUserStore {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            path: cache_dir.into().join(REMEMBERED_FILE),
        }
    }

    fn load_state(&self) -> Result<RememberedState, StorageError> {
        let Some(text) = state_file::read(&self.path)? else {
            return Ok(RememberedState::default());
        };
        let state = toml::from_str::<RememberedState>(&text)
            .map_err(|source| StorageError::Invalid(source.to_string()))?;
        state.validate()?;
        Ok(state)
    }

    fn store_state(&self, state: &RememberedState) -> Result<(), StorageError> {
        state.validate()?;
        let text = toml::to_string_pretty(state)
            .map_err(|source| StorageError::Invalid(source.to_string()))?;

        if text.len() > state_file::MAX_STATE_BYTES {
            return Err(StorageError::Invalid("remembered state file is too large".into()));
        }

        let parent = self.path.parent().ok_or_else(|| {
            StorageError::Invalid("remembered state path has no parent".to_string())
        })?;
        ensure_private_parent_dir(parent)?;

        crate::atomic_file::write(
            &self.path,
            text.as_bytes(),
            crate::atomic_file::AtomicWriteOptions {
                mode: 0o600,
                sync_file: true,
                sync_parent: true,
            },
        )
        .map_err(io_error)?;
        set_private_permissions(&self.path)?;
        Ok(())
    }
}

impl UserStore for FileUserStore {
    fn load_remembered_username(&self) -> Result<Option<String>, StorageError> {
        Ok(self.load_state()?.username)
    }

    fn store_remembered_username(&self, username: &str) -> Result<(), StorageError> {
        validate_value("username", username)?;
        let mut state = self.load_state()?;
        state.username = Some(username.to_string());
        self.store_state(&state)
    }

    fn load_remembered_session(&self) -> Result<Option<String>, StorageError> {
        Ok(self.load_state()?.session_id)
    }

    fn store_remembered_session(&self, session_id: &str) -> Result<(), StorageError> {
        validate_value("session_id", session_id)?;
        let mut state = self.load_state()?;
        state.session_id = Some(session_id.to_string());
        self.store_state(&state)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct RememberedState {
    username: Option<String>,
    session_id: Option<String>,
}

impl RememberedState {
    fn validate(&self) -> Result<(), StorageError> {
        if let Some(username) = &self.username {
            validate_value("username", username)?;
        }
        if let Some(session_id) = &self.session_id {
            validate_value("session_id", session_id)?;
        }
        Ok(())
    }
}

fn validate_value(field: &str, value: &str) -> Result<(), StorageError> {
    if value.is_empty() {
        return Err(StorageError::Invalid(format!("{field} must not be empty")));
    }
    if value.len() > 256 {
        return Err(StorageError::Invalid(format!("{field} is too long")));
    }
    if value.chars().any(|ch| ch == '\0' || ch.is_control()) {
        return Err(StorageError::Invalid(format!(
            "{field} must not contain control characters"
        )));
    }
    Ok(())
}

fn ensure_private_parent_dir(parent: &Path) -> Result<(), StorageError> {
    fs::create_dir_all(parent).map_err(io_error)?;
    let metadata = fs::symlink_metadata(parent).map_err(io_error)?;
    if !metadata.file_type().is_dir() {
        return Err(StorageError::Invalid(format!(
            "{} is not a real directory",
            parent.display()
        )));
    }
    set_private_dir_permissions(parent)
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io_error)
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<(), StorageError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> Result<(), StorageError> {
    Ok(())
}

fn io_error(source: std::io::Error) -> StorageError {
    StorageError::Io(source.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "rsdm-store-test-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn remembers_username_and_session() {
        let dir = temp_dir();
        let store = FileUserStore::new(&dir);
        assert_eq!(store.load_remembered_username().unwrap(), None);

        store.store_remembered_username("alice").unwrap();
        store.store_remembered_session("niri").unwrap();
        assert_eq!(
            store.load_remembered_username().unwrap(),
            Some("alice".to_string())
        );
        assert_eq!(
            store.load_remembered_session().unwrap(),
            Some("niri".to_string())
        );

        // A second store re-reads from disk.
        let reopened = FileUserStore::new(&dir);
        assert_eq!(
            reopened.load_remembered_username().unwrap(),
            Some("alice".to_string())
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_control_characters() {
        let dir = temp_dir();
        let store = FileUserStore::new(&dir);
        assert!(store.store_remembered_username("bad\nname").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}

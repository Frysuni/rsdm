use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use rsdm_core::ports::{StorageError, UserStore};
use serde::{Deserialize, Serialize};

mod state_file;

const REMEMBERED_FILE: &str = "remembered.toml";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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

        let tmp = write_private_temp_file(&self.path, parent, text.as_bytes())?;
        if let Err(error) = fs::rename(&tmp, &self.path) {
            let _ = fs::remove_file(&tmp);
            return Err(io_error(error));
        }
        set_private_permissions(&self.path)?;
        sync_parent_dir(parent)?;
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

fn write_private_temp_file(
    final_path: &Path,
    parent: &Path,
    bytes: &[u8],
) -> Result<PathBuf, StorageError> {
    for _ in 0..16 {
        let tmp = parent.join(temp_file_name(final_path)?);
        match open_private_new_file(&tmp) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                    let _ = fs::remove_file(&tmp);
                    return Err(io_error(error));
                }
                return Ok(tmp);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(error)),
        }
    }
    Err(StorageError::Io(
        "could not create a unique temporary remembered state file".to_string(),
    ))
}

fn open_private_new_file(path: &Path) -> Result<File, std::io::Error> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    set_open_options_mode(&mut options);
    options.open(path)
}

fn temp_file_name(final_path: &Path) -> Result<String, StorageError> {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| StorageError::Invalid("remembered state path has no file name".into()))?;
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    Ok(format!(
        ".{file_name}.tmp.{}.{}.{}",
        std::process::id(),
        nanos,
        counter
    ))
}

fn sync_parent_dir(parent: &Path) -> Result<(), StorageError> {
    File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(io_error)
}

#[cfg(unix)]
fn set_open_options_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_open_options_mode(_options: &mut OpenOptions) {}

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
    use super::*;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "rsdm-store-test-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
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

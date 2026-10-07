//! Private recovery records for units owned by one session generation.

use std::{
    fs::{self, File, OpenOptions}, io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use rsdm_core::domain::{SessionPhase, ShutdownPolicy};
use serde::{Deserialize, Serialize};

use super::{SessionError, deadline::Deadline, identity::{SessionIdentity, valid_generation}};

const MAX_RECORD_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionRecord {
    pub identity: SessionIdentity,
    pub anchor_unit: String,
    pub compositor_unit: Option<String>,
    pub compositor_invocation: Vec<u8>,
    pub provider: String,
    pub owns_targets: bool,
    pub phase: SessionPhase,
    #[serde(default)]
    pub exported_environment: Vec<(String, String)>,
    #[serde(default)]
    pub shutdown_deadline_usec: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AppRecord {
    pub unit: String,
    pub invocation_id: Vec<u8>,
    pub policy: ShutdownPolicy,
    pub deadline_usec: Option<u64>,
    pub quit_started: bool,
}

#[derive(Debug, Clone)]
pub(super) struct Runtime {
    pub path: PathBuf,
    pub generation: String,
}

impl Runtime {
    pub fn create(generation: &str) -> Result<Self, SessionError> {
        let base = runtime_directory()?;
        Self::create_at(&base, generation)
    }

    fn create_at(base: &Path, generation: &str) -> Result<Self, SessionError> {
        let path = session_path(base, generation)?;
        for directory in [base.join("rsdm"), base.join("rsdm/sessions"), path.clone(), path.join("apps")] {
            match fs::create_dir(&directory) {
                Ok(()) => {
                    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
                    sync_directory(&directory)?;
                    sync_directory(directory.parent().expect("runtime directory has a parent"))?;
                },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
                Err(error) => return Err(error.into()),
            }
            verify_directory(&directory)?;
        }
        Ok(Self { path, generation: generation.to_string() })
    }

    pub fn open(generation: &str) -> Result<Option<Self>, SessionError> {
        let base = runtime_directory()?;
        let path = session_path(&base, generation)?;
        if !path.try_exists()? {
            return Ok(None);
        }
        verify_directory(&base.join("rsdm"))?;
        verify_directory(&base.join("rsdm/sessions"))?;
        verify_directory(&path)?;
        verify_directory(&path.join("apps"))?;
        Ok(Some(Self { path, generation: generation.to_string() }))
    }

    pub fn save_session(&self, record: &SessionRecord) -> Result<(), SessionError> {
        if record.identity.generation != self.generation {
            return Err(SessionError::State("session record generation mismatch".into()));
        }
        write_record(&self.path.join("session.toml"), record)
    }

    pub fn session(&self) -> Result<SessionRecord, SessionError> {
        let record: SessionRecord = read_record(&self.path.join("session.toml"))?;
        // SAFETY: geteuid has no preconditions.
        if record.identity.generation != self.generation || record.identity.uid != unsafe { libc::geteuid() }
            || record.anchor_unit != format!("rsdm-session-{}.service", self.generation)
        {
            return Err(SessionError::State("session record generation mismatch".into()));
        }
        Ok(record)
    }

    pub fn save_app(&self, record: &AppRecord) -> Result<(), SessionError> {
        record.policy.validate().map_err(|error| SessionError::State(error.into()))?;
        write_record(&self.app_path(&record.unit)?, record)
    }

    pub fn app(&self, unit: &str) -> Result<AppRecord, SessionError> {
        let record: AppRecord = read_record(&self.app_path(unit)?)?;
        if record.unit != unit {
            return Err(SessionError::State("app record identity mismatch".into()));
        }
        record.policy.validate().map_err(|error| SessionError::State(error.into()))?;
        Ok(record)
    }

    pub fn apps(&self) -> Result<Vec<AppRecord>, SessionError> {
        let mut records = Vec::new();
        for entry in fs::read_dir(self.path.join("apps"))? {
            let entry = entry?;
            if let Some(unit) = entry.file_name().to_str().and_then(|name| name.strip_suffix(".toml")) {
                records.push(self.app(unit)?);
            }
        }
        Ok(records)
    }

    pub fn remove_app(&self, unit: &str) -> Result<(), SessionError> {
        fs::remove_file(self.app_path(unit)?)?;
        Ok(())
    }

    pub fn reap_closed_app_locks(&self) -> Result<usize, SessionError> {
        let directory = self.path.join("apps");
        let mut reaped = 0;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue; };
            let Some(unit) = name.strip_suffix(".lock") else { continue; };
            if directory.join(format!("{unit}.toml")).try_exists()? { continue; }
            if super::app_record_lease::reap_closed(&path)? { reaped += 1; }
        }
        Ok(reaped)
    }

    pub fn app_lease(&self, unit: &str) -> Result<File, SessionError> {
        self.app_lease_until(unit, &Deadline::default())
    }

    pub fn app_lease_until(&self, unit: &str, deadline: &Deadline) -> Result<File, SessionError> {
        let path = self.app_path(unit)?.with_extension("lock");
        super::app_record_lease::acquire(&path, deadline)
    }

    fn app_path(&self, unit: &str) -> Result<PathBuf, SessionError> {
        if !valid_app_unit(unit, &self.generation) {
            return Err(SessionError::State("invalid managed application unit".into()));
        }
        Ok(self.path.join("apps").join(format!("{unit}.toml")))
    }
}

fn runtime_directory() -> Result<PathBuf, SessionError> {
    let path = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from).ok_or_else(|| SessionError::State("XDG_RUNTIME_DIR is required".into()))?;
    verify_directory(&path)?;
    Ok(path)
}

fn session_path(base: &Path, generation: &str) -> Result<PathBuf, SessionError> {
    if !valid_generation(generation) {
        return Err(SessionError::State("invalid session generation".into()));
    }
    verify_directory(base)?;
    Ok(base.join("rsdm/sessions").join(generation))
}

fn verify_directory(path: &Path) -> Result<(), SessionError> {
    let metadata = fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no preconditions.
    if !path.is_absolute() || !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(SessionError::State(format!("unsafe session runtime directory {}", path.display())));
    }
    Ok(())
}

fn valid_app_unit(unit: &str, generation: &str) -> bool {
    unit.starts_with("app-rsdm-") && unit.ends_with(".service") && unit.len() <= 250
        && unit.split_once('@').is_some_and(|(_, instance)| instance.starts_with(&format!("{generation}-")))
        && unit.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'@' | b'.'))
}

fn write_record(path: &Path, record: &impl Serialize) -> Result<(), SessionError> {
    let encoded = toml::to_string(record).map_err(|error| SessionError::State(error.to_string()))?;
    if encoded.len() > MAX_RECORD_BYTES {
        return Err(SessionError::State("session recovery record is too large".into()));
    }
    let parent = path.parent().ok_or_else(|| SessionError::State("recovery record has no parent".into()))?;
    let temporary = path.with_extension(format!("{}.tmp", super::identity::new_generation()?));
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&temporary)?;
        file.write_all(encoded.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        sync_directory(parent)
    })();
    if result.is_err() { let _ = fs::remove_file(&temporary); }
    Ok(result?)
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    OpenOptions::new().read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?.sync_all()
}

fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, SessionError> {
    let file: File = OpenOptions::new().read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path)?;
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.permissions().mode() & 0o077 != 0 {
        return Err(SessionError::State("unsafe session recovery record".into()));
    }
    let mut text = String::new();
    file.take((MAX_RECORD_BYTES + 1) as u64).read_to_string(&mut text)?;
    if text.len() > MAX_RECORD_BYTES { return Err(SessionError::State("session recovery record is too large".into())); }
    toml::from_str(&text).map_err(|error| SessionError::State(error.to_string()))
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;

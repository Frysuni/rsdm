//! Verified runtime state and the privileged emergency-unlock request.
//!
//! Killing an ext-session-lock client cannot unlock the session: the compositor
//! deliberately keeps an abandoned lock active.  The emergency path therefore
//! signals the live rsdm locker, which sends the protocol's normal unlock
//! request itself.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

mod process_identity;
mod state_file;
use process_identity::{pin as pin_process, process_start_time, verify as verify_process};
use state_file::read as read_state_file;

const STATE_VERSION: u8 = 1;
const STATE_DIR: &str = "rsdm";
const STATE_FILE: &str = "lock.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockState {
    pub version: u8,
    pub pid: u32,
    pub uid: u32,
    /// Linux `/proc/<pid>/stat` field 22, checked after pinning emergency targets.
    pub start_time: u64,
}

/// Removes only the state record created for this exact process incarnation.
#[derive(Debug)]
pub struct LockStateGuard {
    path: PathBuf,
    state: LockState,
}

impl Drop for LockStateGuard {
    fn drop(&mut self) {
        if read_state_file(&self.path, self.state.uid).is_ok_and(|state| state == self.state) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Publish that the current process owns a compositor-confirmed session lock.
pub fn register_lock() -> Result<LockStateGuard, LockControlError> {
    let uid = current_uid();
    let pid = std::process::id();
    let state = LockState {
        version: STATE_VERSION,
        pid,
        uid,
        start_time: process_start_time(pid)?,
    };
    let dir = runtime_dir(uid);
    fs::create_dir_all(&dir).map_err(|source| LockControlError::Io {
        path: dir.clone(),
        source,
    })?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|source| {
        LockControlError::Io {
            path: dir.clone(),
            source,
        }
    })?;

    let path = dir.join(STATE_FILE);
    let temporary = dir.join(format!(".{STATE_FILE}.{pid}.{}.tmp", state.start_time));
    let encoded = toml::to_string(&state).map_err(LockControlError::Encode)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|source| LockControlError::Io {
            path: temporary.clone(),
            source,
        })?;
    if let Err(source) = file
        .write_all(encoded.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(LockControlError::Io {
            path: temporary,
            source,
        });
    }
    if let Err(source) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        return Err(LockControlError::Io {
            path: path.clone(),
            source,
        });
    }
    Ok(LockStateGuard { path, state })
}

/// Return verified live lock state for `uid`; stale or forged records are errors.
pub fn lock_state(uid: u32) -> Result<Option<LockState>, LockControlError> {
    let path = state_path(uid);
    let state = match read_state_file(&path, uid) {
        Ok(state) => state,
        Err(LockControlError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    verify_process(&state, uid)?;
    Ok(Some(state))
}

/// Ask a live rsdm locker to perform the protocol's normal unlock operation.
pub fn request_emergency_unlock(
    uid: u32,
    timeout: Duration,
) -> Result<LockState, LockControlError> {
    if current_uid() != 0 {
        return Err(LockControlError::RootRequired);
    }
    let state = lock_state(uid)?.ok_or(LockControlError::NotLocked(uid))?;
    let process = pin_process(&state, uid)?;
    process.signal(libc::SIGUSR1).map_err(LockControlError::Signal)?;

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match lock_state(uid) {
            Ok(None) => return Ok(state),
            Ok(Some(current)) if current != state => return Ok(state),
            Ok(Some(_)) => thread::sleep(Duration::from_millis(50)),
            // The process may have removed the file between metadata/read checks.
            Err(LockControlError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(state);
            }
            Err(error) => return Err(error),
        }
    }
    Err(LockControlError::TimedOut(state.pid))
}

/// UIDs with a verified active rsdm lock. Used when root did not name a user.
pub fn locked_users() -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/run/user") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
        .filter(|uid| lock_state(*uid).ok().flatten().is_some())
        .collect()
}

pub fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

fn runtime_dir(uid: u32) -> PathBuf {
    if uid == current_uid()
        && let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR")
    {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return dir.join(STATE_DIR);
        }
    }
    PathBuf::from(format!("/run/user/{uid}/{STATE_DIR}"))
}

fn state_path(uid: u32) -> PathBuf {
    runtime_dir(uid).join(STATE_FILE)
}

#[derive(Debug, Error)]
pub enum LockControlError {
    #[error("emergency unlock requires root privileges")]
    RootRequired,
    #[error("user with uid {0} has no active rsdm lock")]
    NotLocked(u32),
    #[error("refusing unsafe lock state file {path}", path = .0.display())]
    UnsafeState(PathBuf),
    #[error("lock state does not match live rsdm lock process {0}")]
    ProcessMismatch(u32),
    #[error("invalid procfs data for process {0}")]
    InvalidProc(u32),
    #[error("unsupported lock state version {0}")]
    UnsupportedVersion(u8),
    #[error("failed to signal the lock process: {0}")]
    Signal(std::io::Error),
    #[error("lock process {0} did not acknowledge emergency unlock in time")]
    TimedOut(u32),
    #[error("I/O error at {path}: {source}", path = .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("encoding lock state: {0}")]
    Encode(toml::ser::Error),
    #[error("decoding lock state: {0}")]
    Decode(toml::de::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_own_proc_identity() {
        let pid = std::process::id();
        assert_eq!(process_identity::process_uid(pid).expect("uid"), current_uid());
        assert!(process_start_time(pid).expect("start time") > 0);
    }

    #[test]
    fn state_round_trips() {
        let state = LockState {
            version: STATE_VERSION,
            pid: 42,
            uid: 1000,
            start_time: 1234,
        };
        let encoded = toml::to_string(&state).expect("encode");
        assert_eq!(
            toml::from_str::<LockState>(&encoded).expect("decode"),
            state
        );
    }
}

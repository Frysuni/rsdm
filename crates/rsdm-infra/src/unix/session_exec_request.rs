//! Private, immutable handoff of session parameters to the fresh exec helper.

use std::{fs::File, io::{self, Read, Seek, SeekFrom, Write}, os::fd::{AsRawFd, FromRawFd}};

use rsdm_core::ports::SessionLaunchError;
use serde::{Deserialize, Serialize};

use super::command::{PreparedCommand, cstring};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const SEALS: libc::c_int = libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecRequest {
    pub username: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub command: Vec<String>,
    pub wrapper: Vec<String>,
    pub environment: Vec<(String, String)>,
}

impl ExecRequest {
    pub fn validate(&self) -> Result<(), SessionLaunchError> {
        cstring("username", &self.username)?;
        cstring("home", &self.home)?;
        PreparedCommand::new_wrapped_argv(&self.wrapper, &self.command)?;
        for (key, value) in &self.environment {
            if key.is_empty() || key.contains('=') { return Err(setup("invalid environment key")); }
            cstring("env key", key)?;
            cstring("env value", value)?;
        }
        Ok(())
    }

    pub fn seal(&self) -> Result<File, SessionLaunchError> {
        self.validate()?;
        let text = toml::to_string(self).map_err(setup)?;
        if text.len() > MAX_REQUEST_BYTES { return Err(setup("session exec request exceeds 1 MiB")); }
        let mut file = memfd().map_err(setup)?;
        file.write_all(text.as_bytes()).map_err(setup)?;
        file.seek(SeekFrom::Start(0)).map_err(setup)?;
        // SAFETY: this private memfd is not mapped and no other writer exists.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS, SEALS) } < 0 {
            return Err(setup(io::Error::last_os_error()));
        }
        Ok(file)
    }

    pub fn read(mut file: File) -> Result<Self, SessionLaunchError> {
        // Require a sealed seekable file before reading; a pipe or mutable file
        // cannot make the privileged helper wait for attacker-controlled input.
        let seals = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
        if seals < 0 || seals & SEALS != SEALS { return Err(setup("session exec request is not sealed")); }
        if file.metadata().map_err(setup)?.len() > MAX_REQUEST_BYTES as u64 {
            return Err(setup("session exec request exceeds 1 MiB"));
        }
        file.seek(SeekFrom::Start(0)).map_err(setup)?;
        let mut text = String::new();
        file.take(MAX_REQUEST_BYTES as u64 + 1).read_to_string(&mut text).map_err(setup)?;
        let request: Self = toml::from_str(&text).map_err(setup)?;
        request.validate()?;
        Ok(request)
    }
}

fn memfd() -> io::Result<File> {
    let flags = libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING;
    // SAFETY: the constant name is terminated; no executable mapping is needed.
    let mut fd = unsafe { libc::memfd_create(c"rsdm-session-exec".as_ptr(), flags | libc::MFD_NOEXEC_SEAL) };
    if fd < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL) {
        // MFD_NOEXEC_SEAL is unavailable on older pidfd-capable kernels.
        fd = unsafe { libc::memfd_create(c"rsdm-session-exec".as_ptr(), flags) };
    }
    if fd < 0 { return Err(io::Error::last_os_error()); }
    // SAFETY: memfd_create returned a new descriptor owned by this file.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn setup(error: impl std::fmt::Display) -> SessionLaunchError {
    SessionLaunchError::Setup(error.to_string())
}

#[cfg(test)]
#[path = "session_exec_request_tests.rs"]
mod tests;

//! Verify that the pinned emergency target still belongs to this locker.

use std::{fs, path::PathBuf};

use crate::unix::process_handle::ProcessHandle;
use super::{LockControlError, LockState};

pub(super) fn pin(state: &LockState, uid: u32) -> Result<ProcessHandle, LockControlError> {
    let process = ProcessHandle::open(state.pid).map_err(LockControlError::Signal)?
        .ok_or(LockControlError::ProcessMismatch(state.pid))?;
    // The procfs reads must describe the process pinned above. Check survival
    // afterward so PID replacement during validation cannot pass these checks.
    verify(state, uid)?;
    if !process.alive().map_err(LockControlError::Signal)? {
        return Err(LockControlError::ProcessMismatch(state.pid));
    }
    Ok(process)
}

pub(super) fn verify(state: &LockState, expected_uid: u32) -> Result<(), LockControlError> {
    if state.uid != expected_uid || process_uid(state.pid)? != expected_uid {
        return Err(LockControlError::ProcessMismatch(state.pid));
    }
    if process_start_time(state.pid)? != state.start_time {
        return Err(LockControlError::ProcessMismatch(state.pid));
    }
    let exe_path = PathBuf::from(format!("/proc/{}/exe", state.pid));
    let exe = fs::read_link(&exe_path)
        .map_err(|source| LockControlError::Io { path: exe_path, source })?;
    if !matches!(exe.file_name().and_then(|name| name.to_str()), Some("rsdm" | "rsdm (deleted)")) {
        return Err(LockControlError::ProcessMismatch(state.pid));
    }
    let cmdline_path = PathBuf::from(format!("/proc/{}/cmdline", state.pid));
    let cmdline = fs::read(&cmdline_path)
        .map_err(|source| LockControlError::Io { path: cmdline_path, source })?;
    if !cmdline.split(|byte| *byte == 0).any(|arg| arg == b"lock") {
        return Err(LockControlError::ProcessMismatch(state.pid));
    }
    Ok(())
}

pub(super) fn process_uid(pid: u32) -> Result<u32, LockControlError> {
    let path = PathBuf::from(format!("/proc/{pid}/status"));
    let status = fs::read_to_string(&path).map_err(|source| LockControlError::Io { path, source })?;
    status.lines().find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|uids| uids.split_whitespace().nth(1))
        .and_then(|uid| uid.parse().ok()).ok_or(LockControlError::InvalidProc(pid))
}

pub(super) fn process_start_time(pid: u32) -> Result<u64, LockControlError> {
    let path = PathBuf::from(format!("/proc/{pid}/stat"));
    let stat = fs::read_to_string(&path).map_err(|source| LockControlError::Io { path, source })?;
    let after_comm = stat.rfind(')').and_then(|end| stat.get(end + 1..))
        .ok_or(LockControlError::InvalidProc(pid))?;
    // The remaining fields begin at field 3 (state), so field 22 is index 19.
    after_comm.split_whitespace().nth(19).and_then(|field| field.parse().ok())
        .ok_or(LockControlError::InvalidProc(pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinning_does_not_accept_a_stale_or_wrong_user_identity() {
        let pid = std::process::id();
        let uid = super::super::current_uid();
        let mut state = LockState { version: super::super::STATE_VERSION, pid, uid, start_time: 0 };
        assert!(matches!(pin(&state, uid), Err(LockControlError::ProcessMismatch(_))));
        state.start_time = process_start_time(pid).unwrap();
        assert!(matches!(pin(&state, uid.wrapping_add(1)), Err(LockControlError::ProcessMismatch(_))));
    }
}

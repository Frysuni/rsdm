//! A migrated scope must contain the pinned original process and its children.

use crate::{SessionError, app_origin::OriginalProcess, processes::{self, ProcessHandle}};

pub(super) fn pin_owned(pids: &[u32], group: &str, origin: &OriginalProcess) -> Result<Option<Vec<ProcessHandle>>, SessionError> {
    if !group.starts_with('/') || group == "/" || !pids.contains(&origin.pid) || origin.pidfd_id == 0 {
        return Ok(None);
    }
    let Some(original) = ProcessHandle::open(origin.pid)? else { return Ok(None); };
    if original.id()? != Some(origin.pidfd_id) || !original.alive()? { return Ok(None); }
    let mut handles = Vec::new();
    for &pid in pids {
        let Some(handle) = ProcessHandle::open(pid)? else { return Ok(None); };
        if !descendant(pid, &handle, origin)? { return Ok(None); }
        let membership = match std::fs::read_to_string(format!("/proc/{pid}/cgroup")) {
            Ok(membership) => membership,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !processes::belongs_to_cgroup(&membership, group) || !handle.alive()? { return Ok(None); }
        handles.push(handle);
    }
    if !original.alive()? { return Ok(None); }
    Ok(Some(handles))
}

fn descendant(pid: u32, pinned: &ProcessHandle, origin: &OriginalProcess) -> Result<bool, SessionError> {
    let Some(mut current) = ProcessHandle::open(pid)? else { return Ok(false); };
    if current.id()? != pinned.id()? || !pinned.alive()? { return Ok(false); }
    let mut pid = pid;
    let mut ancestors = Vec::new();
    // A bounded walk rejects ancestry that cannot be established promptly.
    for _ in 0..64 {
        if pid == origin.pid {
            if origin.pidfd_id == 0 || current.id()? != Some(origin.pidfd_id) { return Ok(false); }
            for ancestor in ancestors { if !ProcessHandle::alive(&ancestor)? { return Ok(false); } }
            return Ok(current.alive()? && pinned.alive()?);
        }
        let Some(parent) = parent_pid(pid)? else { return Ok(false); };
        let Some(handle) = ProcessHandle::open(parent)? else { return Ok(false); };
        if parent_pid(pid)? != Some(parent) || !current.alive()? || !handle.alive()? { return Ok(false); }
        ancestors.push(current);
        current = handle;
        pid = parent;
    }
    Ok(false)
}

fn parent_pid(pid: u32) -> Result<Option<u32>, SessionError> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    // The command name can itself contain spaces and parentheses.
    let parent = stat.rfind(')').and_then(|end| stat.get(end + 1..))
        .and_then(|fields| fields.split_whitespace().nth(1)).and_then(|parent| parent.parse().ok());
    Ok(parent.filter(|parent| *parent != 0 && *parent != pid))
}

#[cfg(test)]
#[path = "app_scope_lineage_tests.rs"]
mod tests;

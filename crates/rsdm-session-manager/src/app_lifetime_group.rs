//! Pin cgroup observations without opening a replacement group by its name.

use std::{fs::{self, File}, os::unix::fs::{FileExt, MetadataExt}, path::Path};

use serde::{Deserialize, Serialize};

use crate::SessionError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GroupIdentity {
    pub path: String,
    pub inode: u64,
}

pub(super) struct Group {
    pub identity: GroupIdentity,
    events: File,
    processes: File,
}

impl Group {
    pub(super) fn open(path: &str) -> Result<Self, SessionError> {
        if !path.starts_with('/') || path == "/" || Path::new(path).components().any(|part| matches!(part, std::path::Component::ParentDir)) {
            return Err(SessionError::State("application has no dedicated lifetime cgroup".into()));
        }
        let directory = format!("/sys/fs/cgroup{path}");
        let events = File::open(format!("{directory}/cgroup.events"))?;
        let inode = events.metadata()?.ino();
        let processes = File::open(format!("{directory}/cgroup.procs"))?;
        Ok(Self { identity: GroupIdentity { path: path.into(), inode }, events, processes })
    }

    pub(super) fn restore(identity: &GroupIdentity) -> Result<Option<Self>, SessionError> {
        match Self::open(&identity.path) {
            Ok(group) if group.identity == *identity => Ok(Some(group)),
            Ok(_) => Ok(None),
            Err(SessionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) fn populated(&self) -> Result<bool, SessionError> {
        Ok(read(&self.events)?.lines().any(|line| line == "populated 1"))
    }

    pub(super) fn contains_other_processes(&self, own_pid: u32) -> Result<bool, SessionError> {
        if read(&self.processes)?.split_whitespace().any(|pid| pid.parse::<u32>().ok() != Some(own_pid)) {
            return Ok(true);
        }
        // cgroup.procs covers this directory; children can contain delegated
        // application processes even when the only direct member is our guard.
        for entry in fs::read_dir(format!("/sys/fs/cgroup{}", self.identity.path))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() { continue; }
            match File::open(entry.path().join("cgroup.events")) {
                Ok(events) if read(&events)?.lines().any(|line| line == "populated 1") => return Ok(true),
                Ok(_) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(error.into()),
            }
        }
        Ok(false)
    }
}

pub(super) fn process_group(pid: u32) -> Result<String, SessionError> {
    fs::read_to_string(format!("/proc/{pid}/cgroup"))?.lines().find_map(|line| line.strip_prefix("0::").map(str::to_owned))
        .ok_or_else(|| SessionError::State("application lifetime needs a unified cgroup".into()))
}

fn read(file: &File) -> Result<String, SessionError> {
    let mut bytes = Vec::new();
    loop {
        let mut buffer = [0; 4096];
        let length = match file.read_at(&mut buffer, bytes.len() as u64) {
            Ok(0) => break,
            Ok(length) => length,
            Err(error) if matches!(error.raw_os_error(), Some(libc::ENODEV | libc::ENOENT)) => return Ok(String::new()),
            Err(error) => return Err(error.into()),
        };
        bytes.extend_from_slice(&buffer[..length]);
        if bytes.len() > 1_048_576 { return Err(SessionError::State("application cgroup observation is too large".into())); }
    }
    String::from_utf8(bytes).map_err(|error| SessionError::State(error.to_string()))
}

#[cfg(test)]
#[path = "app_lifetime_group_tests.rs"]
mod tests;

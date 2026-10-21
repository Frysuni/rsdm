//! Filesystem paths must retain their exact identity when sent over D-Bus.

use std::path::Path;

use crate::SessionError;

pub(super) fn encode(path: &Path) -> Result<String, SessionError> {
    path.to_str().map(str::to_owned)
        .ok_or_else(|| SessionError::State("filesystem path is not valid UTF-8".into()))
}

pub(super) fn working_directory(path: &Path) -> Result<String, SessionError> {
    let directory = encode(path)?;
    let leaf = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    let backslashes = leaf.bytes().rev().take_while(|byte| *byte == b'\\').count();
    // systemd writes WorkingDirectory into an unquoted transient unit setting.
    if directory.contains(['\n', '\r']) || leaf.ends_with([' ', '\t']) || backslashes % 2 != 0 {
        return Err(SessionError::State(
            "working directory cannot be preserved across systemd reloads".into(),
        ));
    }
    Ok(directory)
}

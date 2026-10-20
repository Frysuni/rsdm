//! Filesystem paths must retain their exact identity when sent over D-Bus.

use std::path::Path;

use crate::SessionError;

pub(super) fn encode(path: &Path) -> Result<String, SessionError> {
    path.to_str().map(str::to_owned)
        .ok_or_else(|| SessionError::State("filesystem path is not valid UTF-8".into()))
}

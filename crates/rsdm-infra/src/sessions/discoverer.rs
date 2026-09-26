use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use rsdm_core::{
    domain::{DmConfig, Session},
    ports::{SessionDiscoverer, SessionDiscoveryError},
};

use super::parse_desktop_entry;

#[derive(Debug, Clone)]
pub struct DesktopSessionDiscoverer {
    paths: Vec<PathBuf>,
}

impl DesktopSessionDiscoverer {
    fn new(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            paths: paths.into_iter().collect(),
        }
    }

    pub fn from_config(config: &DmConfig) -> Self {
        Self::new(config.session_dirs.iter().map(PathBuf::from))
    }
}

impl SessionDiscoverer for DesktopSessionDiscoverer {
    fn discover(&self) -> Result<Vec<Session>, SessionDiscoveryError> {
        let mut sessions = Vec::new();
        let mut seen_ids = HashSet::new();
        tracing::debug!(paths = ?self.paths, "discovering desktop sessions");

        for dir in &self.paths {
            if !dir.exists() {
                tracing::debug!(path = %dir.display(), "session directory does not exist");
                continue;
            }

            let files = desktop_files(dir)?;
            tracing::debug!(
                path = %dir.display(),
                desktop_file_count = files.len(),
                "session directory scanned"
            );
            for path in files {
                let Some(id) = session_id(&path) else {
                    continue;
                };
                // Even a hidden or unavailable override masks lower-priority entries.
                if !seen_ids.insert(id) {
                    continue;
                }
                let Some(session) = parse_session_file(&path)? else {
                    continue;
                };
                tracing::debug!(
                    id = %session.id,
                    name = %session.name,
                    exec = %session.exec,
                    source = %session.source_path,
                    "desktop session discovered"
                );
                sessions.push(session);
            }
        }

        sessions.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        tracing::info!(count = sessions.len(), "desktop session discovery complete");
        Ok(sessions)
    }
}

fn desktop_files(dir: &Path) -> Result<Vec<PathBuf>, SessionDiscoveryError> {
    let mut files = Vec::new();
    let entries = fs::read_dir(dir).map_err(|source| {
        SessionDiscoveryError::Backend(format!("failed to read {}: {source}", dir.display()))
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| {
            SessionDiscoveryError::Backend(format!("failed to read {}: {source}", dir.display()))
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("desktop") {
            files.push(path);
        }
    }

    files.sort();
    Ok(files)
}

fn parse_session_file(path: &Path) -> Result<Option<Session>, SessionDiscoveryError> {
    let Ok(text) = fs::read_to_string(path) else {
        tracing::debug!(path = %path.display(), "failed to read desktop session file; skipping");
        return Ok(None);
    };
    let entry = match parse_desktop_entry(&text) {
        Ok(entry) => entry,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "failed to parse desktop session file; skipping");
            return Ok(None);
        }
    };
    if let Some(try_exec) = &entry.try_exec
        && !try_exec_available(try_exec)
    {
        tracing::debug!(
            path = %path.display(),
            %try_exec,
            "desktop session TryExec is unavailable; skipping"
        );
        return Ok(None);
    }
    let Some(id) = session_id(path) else {
        tracing::debug!(path = %path.display(), "desktop session file has no valid id; skipping");
        return Ok(None);
    };

    Ok(entry.to_session(&id, &path.display().to_string()))
}

fn session_id(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
}

fn try_exec_available(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }

    let path = Path::new(value);
    if path.components().count() > 1 {
        return is_executable(path);
    }

    env::var_os("PATH")
        .and_then(|paths| env::split_paths(&paths).find(|dir| is_executable(&dir.join(value))))
        .is_some()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_override_masks_lower_priority_session() {
        let root = env::temp_dir().join(format!("rsdm-session-priority-{}", std::process::id()));
        let high = root.join("high");
        let low = root.join("low");
        fs::create_dir_all(&high).unwrap();
        fs::create_dir_all(&low).unwrap();
        fs::write(high.join("test.desktop"), "[Desktop Entry]\nHidden=true\n").unwrap();
        fs::write(
            low.join("test.desktop"),
            "[Desktop Entry]\nName=Test\nExec=test\n",
        )
        .unwrap();
        let discoverer = DesktopSessionDiscoverer::new([high.clone(), low]);
        assert!(discoverer.discover().unwrap().is_empty());

        fs::remove_file(high.join("test.desktop")).unwrap();
        assert_eq!(discoverer.discover().unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}

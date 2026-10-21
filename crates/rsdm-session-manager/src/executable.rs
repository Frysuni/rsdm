//! Resolve the installed executable path for commands started by the user manager.

use std::{fs, io, os::unix::{ffi::OsStrExt, fs::MetadataExt}, path::PathBuf};

pub(super) fn installed_path() -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    if !executable.as_os_str().as_bytes().ends_with(b" (deleted)") {
        return Ok(executable);
    }

    let running = fs::metadata("/proc/self/exe")?;
    match fs::metadata(&executable) {
        Ok(candidate) if candidate.dev() == running.dev() && candidate.ino() == running.ino() => return Ok(executable),
        Ok(_) => {},
        Err(error) if error.kind() == io::ErrorKind::NotFound => {},
        Err(error) => return Err(error),
    }

    // Re-read after checking identity: a literal suffix may have been unlinked
    // during the check, in which case the kernel now appends a second suffix.
    let executable = std::env::current_exe()?;
    match executable.as_os_str().as_bytes().strip_suffix(b" (deleted)") {
        Some(original) => Ok(PathBuf::from(std::ffi::OsStr::from_bytes(original))),
        None => Ok(executable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHILD: &str = "RSDM_EXECUTABLE_PATH_TEST_CHILD";
    const TEST: &str = "executable::tests::replacement_preserves_the_installed_path_including_a_literal_deleted_suffix";

    #[test]
    fn replacement_preserves_the_installed_path_including_a_literal_deleted_suffix() {
        let child = std::env::var_os(CHILD);
        if child.is_none() {
            // Parallel tests must not inherit a writable descriptor to the copied executable.
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST]).env(CHILD, "").output().unwrap();
            assert!(output.status.success(), "{output:?}");
            return;
        }
        if let Some(path) = child.filter(|path| !path.is_empty()) {
            let path = PathBuf::from(path);
            assert_eq!(installed_path().unwrap(), path);
            // Another hard link keeps nlink nonzero after the executable is replaced.
            fs::hard_link(&path, path.with_extension("old")).unwrap();
            fs::write(path.with_extension("next"), "replacement").unwrap();
            fs::rename(path.with_extension("next"), &path).unwrap();
            let mut annotated = path.as_os_str().to_owned();
            annotated.push(" (deleted)");
            fs::write(annotated, "unrelated file").unwrap();
            assert_eq!(installed_path().unwrap(), path);
            return;
        }

        let directory = std::env::temp_dir().join(format!("rsdm-executable-path-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        for filename in ["rsdm", "rsdm (deleted)"] {
            let path = directory.join(filename);
            fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
            let output = std::process::Command::new(&path)
                .args(["--exact", TEST])
                .env(CHILD, &path).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            fs::remove_dir_all(&directory).unwrap();
            fs::create_dir(&directory).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }
}

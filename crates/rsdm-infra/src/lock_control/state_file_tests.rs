use std::{fs, os::unix::fs::{PermissionsExt, symlink}, path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering}};

use super::*;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rsdm-lock-state-test-{}-{}",
            std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn state(&self, name: &str, pid: u32) -> PathBuf {
        let path = self.0.join(name);
        let state = LockState { version: STATE_VERSION, pid, uid: super::super::current_uid(), start_time: 123 };
        fs::write(&path, toml::to_string(&state).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
}

impl Drop for Directory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

#[test]
fn replacing_the_path_does_not_replace_the_open_record() {
    let directory = Directory::new();
    let path = directory.state("lock.toml", 42);
    let file = File::open(&path).unwrap();
    let replacement = directory.state("replacement.toml", 43);
    fs::rename(replacement, &path).unwrap();
    let uid = super::super::current_uid();
    assert_eq!(read_file(file, &path, uid).unwrap().pid, 42);
    assert_eq!(read(&path, uid).unwrap().pid, 43);
}

#[test]
fn symlinks_fifos_and_shared_records_are_rejected() {
    let directory = Directory::new();
    let path = directory.state("lock.toml", 42);
    let uid = super::super::current_uid();
    let link = directory.0.join("link");
    symlink(&path, &link).unwrap();
    assert!(read(&link, uid).is_err());
    let fifo = directory.0.join("fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: name is terminated and mkfifo retains no pointer.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(read(&fifo, uid).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o622)).unwrap();
    assert!(matches!(read(&path, uid), Err(LockControlError::UnsafeState(_))));
}

#[test]
fn oversized_and_wrong_owner_records_are_rejected() {
    let directory = Directory::new();
    let path = directory.state("lock.toml", 42);
    let uid = super::super::current_uid();
    assert!(matches!(read(&path, uid.wrapping_add(1)), Err(LockControlError::UnsafeState(_))));
    fs::write(&path, "x".repeat(MAX_STATE_BYTES as usize + 1)).unwrap();
    assert!(matches!(read(&path, uid), Err(LockControlError::UnsafeState(_))));
}

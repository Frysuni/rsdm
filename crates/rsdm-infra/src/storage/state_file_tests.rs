use std::{
    ffi::CString,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use rsdm_core::ports::UserStore;

use super::*;
use super::super::FileUserStore;

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rsdm-state-read-test-{}-{}", std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn missing_state_and_normal_writes_round_trip() {
    let directory = Directory::new();
    let store = FileUserStore::new(&directory.0);
    assert_eq!(store.load_remembered_username().unwrap(), None);
    store.store_remembered_username("alice").unwrap();
    store.store_remembered_session("session").unwrap();
    assert_eq!(store.load_remembered_username().unwrap().as_deref(), Some("alice"));
    assert_eq!(store.load_remembered_session().unwrap().as_deref(), Some("session"));
}

#[test]
fn pathname_replacement_cannot_replace_an_opened_file() {
    let directory = Directory::new();
    let path = directory.file("remembered.toml", "username = 'alice'\n");
    let file = File::open(&path).unwrap();
    let replacement = directory.file("replacement", "username = 'bob'\n");
    fs::rename(replacement, &path).unwrap();
    assert_eq!(read_file(file).unwrap(), "username = 'alice'\n");
    assert_eq!(read(&path).unwrap().unwrap(), "username = 'bob'\n");
}

#[test]
fn symlinks_including_dangling_links_are_errors() {
    let directory = Directory::new();
    let target = directory.file("target", "username = 'alice'\n");
    let path = directory.0.join("remembered.toml");
    symlink(&target, &path).unwrap();
    let store = FileUserStore::new(&directory.0);
    assert!(store.load_remembered_username().is_err());
    assert!(store.store_remembered_username("bob").is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "username = 'alice'\n");
    fs::remove_file(target).unwrap();
    assert!(store.load_remembered_username().is_err());
}

#[test]
fn fifos_and_directories_are_rejected_without_reading() {
    let directory = Directory::new();
    let path = directory.0.join("remembered.toml");
    let name = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: name is terminated and mkfifo retains no pointer.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(matches!(read(&path), Err(StorageError::Invalid(_))));
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(matches!(read(&path), Err(StorageError::Invalid(_))));
}

#[test]
fn shared_permissions_are_rejected() {
    let directory = Directory::new();
    let path = directory.file("remembered.toml", "username = 'alice'\n");
    for mode in [0o640, 0o620, 0o604, 0o602] {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        assert!(matches!(read(&path), Err(StorageError::Invalid(_))));
    }
}

#[test]
fn exact_size_is_accepted_but_oversized_state_is_rejected() {
    let directory = Directory::new();
    let prefix = "username = 'alice'\n#";
    let text = format!("{prefix}{}", "x".repeat(MAX_STATE_BYTES - prefix.len()));
    let path = directory.file("remembered.toml", &text);
    let store = FileUserStore::new(&directory.0);
    assert_eq!(store.load_remembered_username().unwrap().as_deref(), Some("alice"));
    let file = OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(MAX_STATE_BYTES as u64 + 1).unwrap();
    assert!(matches!(read(&path), Err(StorageError::Invalid(_))));
}

#[test]
fn maximum_valid_values_round_trip_with_escaping() {
    let directory = Directory::new();
    let store = FileUserStore::new(&directory.0);
    let value = "\"\\".repeat(128);
    store.store_remembered_username(&value).unwrap();
    store.store_remembered_session(&value).unwrap();
    assert_eq!(store.load_remembered_username().unwrap().as_deref(), Some(value.as_str()));
    assert_eq!(store.load_remembered_session().unwrap().as_deref(), Some(value.as_str()));
}

use std::{
    os::unix::fs::{PermissionsExt, symlink},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("rsdm-session-lease-{}-{id}", std::process::id()));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }

    fn lease_path(&self) -> PathBuf {
        self.0.join("rsdm/session.lock")
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn another_generation_or_cleanup_cannot_replace_a_live_coordinator() {
    let directory = Directory::new();
    for generation in ["first", "second"] {
        fs::create_dir_all(directory.0.join("rsdm/sessions").join(generation)).unwrap();
    }
    fs::set_permissions(directory.0.join("rsdm"), fs::Permissions::from_mode(0o700)).unwrap();
    let _coordinator = SessionLease::acquire_in(&directory.0).unwrap();

    for _ in ["another generation", "generation cleanup"] {
        assert!(matches!(SessionLease::acquire_in(&directory.0), Err(SessionError::SessionAlreadyActive)));
    }
}

#[test]
fn closing_the_owner_releases_the_same_inode_without_unlinking_it() {
    let directory = Directory::new();
    let owner = SessionLease::acquire_in(&directory.0).unwrap();
    let path = directory.lease_path();
    let inode = fs::metadata(&path).unwrap().ino();
    fs::write(&path, b"retained contents").unwrap();
    drop(owner);

    let _next_owner = SessionLease::acquire_in(&directory.0).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(fs::read(&path).unwrap(), b"retained contents");
}

#[test]
fn the_lease_and_parent_are_private_and_close_on_exec() {
    let directory = Directory::new();
    let lease = SessionLease::acquire_in(&directory.0).unwrap();
    assert_eq!(fs::metadata(directory.0.join("rsdm")).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(directory.lease_path()).unwrap().mode() & 0o777, 0o600);
    // SAFETY: the lease owns a valid descriptor and F_GETFD only reads its flags.
    let flags = unsafe { libc::fcntl(lease._file.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
}

#[test]
fn symlinks_and_shared_directories_cannot_supply_the_lease() {
    let directory = Directory::new();
    let parent = directory.0.join("rsdm");
    let outside = directory.0.join("outside");
    DirBuilder::new().mode(0o700).create(&outside).unwrap();
    symlink(&outside, &parent).unwrap();
    assert!(SessionLease::acquire_in(&directory.0).is_err());
    fs::remove_file(&parent).unwrap();
    DirBuilder::new().mode(0o755).create(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(SessionLease::acquire_in(&directory.0).is_err());
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&outside, directory.lease_path()).unwrap();
    assert!(SessionLease::acquire_in(&directory.0).is_err());
}

#[test]
fn non_regular_or_shared_files_are_rejected_without_blocking() {
    let directory = Directory::new();
    let owner = SessionLease::acquire_in(&directory.0).unwrap();
    drop(owner);
    let path = directory.lease_path();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(SessionLease::acquire_in(&directory.0).is_err());
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(SessionLease::acquire_in(&directory.0).is_err());
    fs::remove_dir(&path).unwrap();

    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: c_path is a terminated pathname retained throughout mkfifo.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    assert!(SessionLease::acquire_in(&directory.0).is_err());
}

#[test]
fn unsafe_runtime_base_is_rejected_before_creating_the_parent() {
    let directory = Directory::new();
    fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(SessionLease::acquire_in(&directory.0).is_err());
    assert!(!directory.0.join("rsdm").exists());
    fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700)).unwrap();
    let alias = directory.0.join("alias");
    symlink(&directory.0, &alias).unwrap();
    assert!(SessionLease::acquire_in(&alias).is_err());
}

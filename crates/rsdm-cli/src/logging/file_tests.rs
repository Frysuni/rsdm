use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use super::*;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        loop {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("rsdm-log-{}-{sequence}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
                    return Self(path);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create log fixture: {error}"),
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); }
}

#[test]
fn log_file_creates_parent_directory_and_appends() {
    let fixture = Fixture::new();
    let path = fixture.0.join("nested/rsdm.log");
    writeln!(open(&path).unwrap(), "first").unwrap();
    writeln!(open(&path).unwrap(), "second").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o022, 0);
}

#[test]
fn rejects_symlinks_without_modifying_the_target() {
    let fixture = Fixture::new();
    let target = fixture.0.join("target");
    fs::write(&target, "untouched").unwrap();
    let path = fixture.0.join("rsdm.log");
    symlink(&target, &path).unwrap();
    assert!(open(&path).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
    fs::remove_file(&path).unwrap();
    symlink(fixture.0.join("missing"), &path).unwrap();
    assert!(open(&path).is_err());
    assert!(!fixture.0.join("missing").exists());
}

#[test]
fn rejects_symlinked_and_shared_writable_parent_directories() {
    let fixture = Fixture::new();
    symlink(&fixture.0, fixture.0.join("alias")).unwrap();
    assert!(open(&fixture.0.join("alias/rsdm.log")).is_err());
    for mode in [0o770, 0o777, 0o1777] {
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(mode)).unwrap();
        assert!(open(&fixture.0.join("rsdm.log")).is_err(), "mode {mode:o}");
        assert!(!fixture.0.join("rsdm.log").exists());
    }
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn rejects_shared_writable_non_sticky_ancestors_before_creating_children() {
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(open(&fixture.0.join("nested/rsdm.log")).is_err());
    assert!(!fixture.0.join("nested").exists());
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn rejects_shared_writable_log_files() {
    let fixture = Fixture::new();
    let path = fixture.0.join("rsdm.log");
    fs::write(&path, "untouched").unwrap();
    for mode in [0o660, 0o646, 0o666] {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        assert!(open(&path).is_err(), "mode {mode:o}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "untouched");
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(open(&path).is_ok());
}

#[test]
fn rejects_fifos_and_directories() {
    let fixture = Fixture::new();
    let fifo = fixture.0.join("fifo");
    let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: name is a valid C string in the isolated private fixture.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    // A reader makes the nonblocking write open succeed, so the FD type check
    // must still reject the FIFO. Without a reader, opening must not hang either.
    assert!(open(&fifo).is_err());
    let _reader = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&fifo).unwrap();
    assert!(open(&fifo).is_err());
    assert!(open(&fixture.0).is_err());
}

#[test]
fn pinned_directory_traversal_ignores_a_replacement_path() {
    let fixture = Fixture::new();
    let original = fixture.0.join("original");
    fs::create_dir(&original).unwrap();
    let pinned = File::open(&original).unwrap();
    let moved = fixture.0.join("moved");
    fs::rename(&original, &moved).unwrap();
    let replacement = fixture.0.join("replacement");
    fs::create_dir(&replacement).unwrap();
    symlink(&replacement, &original).unwrap();
    let child = open_directory(&pinned, &CString::new("nested").unwrap()).unwrap();
    assert!(child.metadata().unwrap().is_dir());
    assert!(moved.join("nested").is_dir());
    assert!(!replacement.join("nested").exists());
}

#[test]
fn rejects_relative_and_parent_traversal_paths() {
    assert!(open(Path::new("rsdm.log")).is_err());
    let fixture = Fixture::new();
    assert!(open(&fixture.0.join("../rsdm.log")).is_err());
}

#[test]
fn rejects_foreign_file_ownership_when_running_as_root() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 { return; }
    let fixture = Fixture::new();
    let path = fixture.0.join("rsdm.log");
    fs::write(&path, "untouched").unwrap();
    let name = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: change only this test's file, with valid IDs and a valid C string.
    assert_eq!(unsafe { libc::chown(name.as_ptr(), 1, 1) }, 0);
    assert!(open(&path).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "untouched");
}

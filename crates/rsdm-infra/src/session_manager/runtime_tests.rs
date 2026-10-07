use std::{os::unix::fs::symlink, sync::atomic::{AtomicUsize, Ordering}};

use super::*;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
const GENERATION: &str = "0123456789abcdef0123456789abcdef";

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rsdm-runtime-test-{}-{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn app() -> AppRecord {
    AppRecord {
        unit: format!("app-rsdm-example@{GENERATION}-1.service"),
        invocation_id: vec![1; 16],
        policy: ShutdownPolicy::default(),
        deadline_usec: None,
        quit_started: false,
    }
}

#[test]
fn recovery_records_are_private_and_preserve_the_policy() {
    let directory = Directory::new();
    let runtime = Runtime::create_at(&directory.0, GENERATION).unwrap();
    let mut record = app();
    record.policy.quit_command = vec!["examplectl".into(), "quit".into()];
    runtime.save_app(&record).unwrap();
    let path = runtime.app_path(&record.unit).unwrap();
    assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(runtime.app(&record.unit).unwrap().policy, record.policy);
    assert_eq!(runtime.apps().unwrap().len(), 1);
}

#[test]
fn app_records_cannot_escape_the_directory_or_own_another_generation() {
    let directory = Directory::new();
    let runtime = Runtime::create_at(&directory.0, GENERATION).unwrap();
    for unit in ["../../other.service", "app-rsdm-example@other-1.service", "other.service"] {
        assert!(runtime.app_path(unit).is_err());
    }
}

#[test]
fn unsafe_parents_and_symlink_records_are_rejected() {
    let directory = Directory::new();
    fs::create_dir(directory.0.join("outside")).unwrap();
    symlink(directory.0.join("outside"), directory.0.join("rsdm")).unwrap();
    assert!(Runtime::create_at(&directory.0, GENERATION).is_err());
    fs::remove_file(directory.0.join("rsdm")).unwrap();
    let runtime = Runtime::create_at(&directory.0, GENERATION).unwrap();
    symlink(directory.0.join("outside"), runtime.app_path(&app().unit).unwrap()).unwrap();
    assert!(runtime.app(&app().unit).is_err());
}

#[test]
fn fifo_records_and_leases_are_rejected_without_waiting_for_a_writer() {
    let directory = Directory::new();
    let runtime = Runtime::create_at(&directory.0, GENERATION).unwrap();
    let record = runtime.app_path(&app().unit).unwrap();
    let lease = record.with_extension("lock");
    for path in [&record, &lease] {
        let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: path is a terminated pathname, and mkfifo retains no pointer.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    }
    assert!(runtime.app(&app().unit).is_err());
    assert!(runtime.app_lease(&app().unit).is_err());
}

#[test]
fn an_atomically_replaced_record_does_not_leave_partial_state() {
    let directory = Directory::new();
    let runtime = Runtime::create_at(&directory.0, GENERATION).unwrap();
    let mut record = app();
    runtime.save_app(&record).unwrap();
    record.deadline_usec = Some(42);
    record.quit_started = true;
    runtime.save_app(&record).unwrap();
    let loaded = runtime.app(&record.unit).unwrap();
    assert_eq!(loaded.deadline_usec, Some(42));
    assert!(loaded.quit_started);
    assert_eq!(fs::read_dir(runtime.path.join("apps")).unwrap().count(), 1);
}

#[test]
fn maximum_sized_records_round_trip_and_oversized_updates_preserve_old_state() {
    let directory = Directory::new();
    let runtime = Runtime::create_at(&directory.0, GENERATION).unwrap();
    let mut record = app();
    record.policy.quit_command = vec!["quit".into(), String::new()];
    let overhead = toml::to_string(&record).unwrap().len();
    record.policy.quit_command[1] = "x".repeat(MAX_RECORD_BYTES - overhead);
    assert_eq!(toml::to_string(&record).unwrap().len(), MAX_RECORD_BYTES);
    runtime.save_app(&record).unwrap();
    assert_eq!(runtime.app(&record.unit).unwrap().policy, record.policy);

    record.policy.quit_command[1].push('x');
    assert!(matches!(runtime.save_app(&record), Err(SessionError::State(message))
        if message == "session recovery record is too large"));
    record.policy.quit_command[1].pop();
    assert_eq!(runtime.app(&record.unit).unwrap().policy, record.policy);
    assert_eq!(fs::read_dir(runtime.path.join("apps")).unwrap().count(), 1);
}

#[test]
fn serialized_size_limit_applies_to_session_records_too() {
    let directory = Directory::new();
    let path = directory.0.join("session.toml");
    let record = SessionRecord {
        identity: SessionIdentity {
            generation: GENERATION.into(), login_session_id: "1".into(),
            desktop_entry_id: None, uid: unsafe { libc::geteuid() },
        },
        anchor_unit: format!("rsdm-session-{GENERATION}.service"),
        compositor_unit: None, compositor_invocation: Vec::new(),
        provider: "managed".into(), owns_targets: true, phase: SessionPhase::Starting,
        exported_environment: vec![("VALUE".into(), "x".repeat(MAX_RECORD_BYTES))],
        shutdown_deadline_usec: None,
    };
    assert!(write_record(&path, &record).is_err());
    assert!(!path.exists());
    assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
}

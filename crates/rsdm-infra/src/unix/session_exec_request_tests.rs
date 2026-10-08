use std::{fs::OpenOptions, io::Write, os::fd::AsRawFd, path::PathBuf};

use super::*;

fn request() -> ExecRequest {
    ExecRequest {
        username: "alice".into(), uid: 1000, gid: 100,
        home: "/home/alice".into(), command: vec!["niri-session".into(), "--session".into()],
        wrapper: vec!["/usr/bin/rsdm".into(), "session".into(), "start".into(), "--".into()],
        environment: vec![("HOME".into(), "/home/alice".into()), ("PATH".into(), "/usr/bin".into())],
    }
}

#[test]
fn sealed_requests_round_trip_without_a_pathname() {
    let original = request();
    let file = original.seal().unwrap();
    assert!(PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())).is_file());
    assert_eq!(ExecRequest::read(file).unwrap(), original);
}

#[test]
fn mutable_files_are_rejected_before_deserialization() {
    let path = std::env::temp_dir().join(format!("rsdm-session-exec-{}", std::process::id()));
    let mut file = OpenOptions::new().create(true).truncate(true).read(true).write(true).open(&path).unwrap();
    file.write_all(b"username = 'alice'\n").unwrap();
    assert!(matches!(ExecRequest::read(file), Err(SessionLaunchError::Setup(message)) if message.contains("sealed")));
    let _ = std::fs::remove_file(path);
}

#[test]
fn invalid_environment_and_commands_are_rejected_before_sealing() {
    let mut invalid = request();
    invalid.environment.push(("BAD=KEY".into(), "value".into()));
    assert!(invalid.seal().is_err());
    let mut invalid = request();
    invalid.command = Vec::new();
    assert!(invalid.seal().is_err());
}

#[test]
fn sealed_request_size_is_bounded() {
    let mut invalid = request();
    invalid.environment.push(("BIG".into(), "x".repeat(1024 * 1024)));
    assert!(invalid.seal().is_err());
}

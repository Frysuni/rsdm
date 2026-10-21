use super::*;

#[test]
fn scoped_lockers_preserve_arguments_with_old_and_new_runners() {
    use std::{ffi::OsStr, fs, os::unix::fs::PermissionsExt};

    const CHILD: &str = "RSDM_IDLE_ARGUMENT_TEST";
    let Ok(mode) = std::env::var(CHILD) else {
        let directory = std::env::temp_dir().join(format!("rsdm-idle-runner-{}", std::process::id()));
        for mode in ["supported", "unsupported", "missing"] {
            let status = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tests::scoped_lockers_preserve_arguments_with_old_and_new_runners", "--nocapture"])
                .env(CHILD, mode).env("INVOCATION_ID", "fixture")
                .env("RSDM_SYSTEMD_RUN", directory.join(mode))
                .status().unwrap();
            assert!(status.success(), "{mode}");
        }
        fs::remove_dir_all(directory).unwrap();
        return;
    };
    let runner = PathBuf::from(std::env::var_os("RSDM_SYSTEMD_RUN").unwrap());
    if mode != "missing" {
        fs::create_dir_all(runner.parent().unwrap()).unwrap();
        fs::write(&runner, "#!/bin/sh\ntest \"$1\" = --expand-environment=no && test \"$2\" = --version || exit 9\ntest \"$RSDM_IDLE_ARGUMENT_TEST\" = supported\n").unwrap();
        fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();
    }

    let argv = ["locker", "$HOME", "$$", "two words"].map(Into::into);
    let result = lock_command(&argv);
    if mode == "missing" { assert!(result.is_none()); return; }
    let (command, scope) = result.unwrap();
    assert!(scope.is_some());
    let arguments = command.get_args().collect::<Vec<_>>();
    assert_eq!(arguments.contains(&OsStr::new("--expand-environment=no")), mode == "supported");
    assert_eq!(&arguments[arguments.len() - argv.len()..], argv.iter().map(|arg| arg.as_os_str()).collect::<Vec<_>>());
}

#[test]
fn lock_worker_guard_resets_activity() {
    let active = Arc::new(AtomicBool::new(true));
    {
        let _guard = ResetActive(Arc::clone(&active));
    }
    assert!(!active.load(Ordering::Acquire));
}

#[test]
fn retry_waits_for_the_failed_worker_to_release_its_slot() {
    let active = Arc::new(AtomicBool::new(true));
    let retry = AtomicBool::new(true);
    let worker = ResetActive(Arc::clone(&active));
    assert!(!take_retry_request(&active, &retry));
    drop(worker);
    assert!(take_retry_request(&active, &retry));
    assert!(!take_retry_request(&active, &retry));
}

#[test]
fn failed_custom_lockers_retry_without_another_idle_notification() {
    const CHILD: &str = "RSDM_IDLE_EXIT_TEST";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::failed_custom_lockers_retry_without_another_idle_notification", "--nocapture"])
            .env(CHILD, "1")
            .env_remove("INVOCATION_ID")
            .env_remove("RSDM_SYSTEMD_RUN")
            .status().unwrap();
        assert!(status.success());
        return;
    }
    for program in ["false", "true"] {
        let config = IdleConfig { lock_command: vec![program.into()], ..IdleConfig::default() };
        let active = Arc::new(AtomicBool::new(true));
        let retry = Arc::new(AtomicBool::new(false));
        run_lock_cycle(config, PathBuf::new(), Arc::clone(&active), Arc::clone(&retry));
        assert!(!active.load(Ordering::Acquire));
        assert_eq!(take_retry_request(&active, &retry), program == "false");
        assert!(!take_retry_request(&active, &retry));
    }
}

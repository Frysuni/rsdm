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

#[test]
fn real_lock_cycles_preserve_hook_order_across_failure_and_relock() {
    use std::fs;

    const CHILD: &str = "RSDM_IDLE_HOOK_CYCLE_TEST";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::real_lock_cycles_preserve_hook_order_across_failure_and_relock", "--nocapture"])
            .env(CHILD, "1")
            .env_remove("INVOCATION_ID")
            .env_remove("RSDM_SYSTEMD_RUN")
            .status().unwrap();
        assert!(status.success());
        return;
    }

    let directory = std::env::temp_dir().join(format!("rsdm-idle-hook-cycle-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let log = directory.join("events");
    let active = Arc::new(AtomicBool::new(true));
    let retry = Arc::new(AtomicBool::new(false));
    let command = |script: &str| {
        vec!["timeout".into(), "10".into(), "sh".into(), "-c".into(), script.into(),
            "fixture".into(), directory.display().to_string()]
    };
    let events = || fs::read_to_string(&log).unwrap_or_default();
    let wait = |condition: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out; events: {}", events());
            thread::sleep(Duration::from_millis(10));
        }
    };

    let first = IdleConfig {
        lock_command: command(r#"while [ ! -e "$1/first-started" ]; do sleep 0.01; done; exit 7"#),
        on_lock: vec![
            command(r#"echo first-start >> "$1/events"; touch "$1/first-started"; while [ ! -e "$1/release-first" ]; do sleep 0.01; done; echo first-end >> "$1/events""#),
            command(r#"echo first-tail >> "$1/events""#),
        ],
        on_unlock: vec![command(r#"echo unexpected-unlock >> "$1/events""#)],
        ..IdleConfig::default()
    };
    let first_active = Arc::clone(&active);
    let first_retry = Arc::clone(&retry);
    let first_worker = thread::spawn(move || run_lock_cycle(first, PathBuf::new(), first_active, first_retry));
    wait(&|| !active.load(Ordering::Acquire));
    assert_eq!(events(), "first-start\n");
    assert!(take_retry_request(&active, &retry));
    assert!(!take_retry_request(&active, &retry));

    // A replacement locker can start while the failed cycle's hook is still running.
    active.store(true, Ordering::Release);
    let second = IdleConfig {
        lock_command: command(r#"while [ ! -e "$1/second-started" ]; do sleep 0.01; done"#),
        on_lock: vec![
            command(r#"echo second-start >> "$1/events"; touch "$1/second-started"; while [ ! -e "$1/release-second" ]; do sleep 0.01; done; echo second-failure >> "$1/events"; exit 4"#),
            vec![directory.join("missing-hook").display().to_string()],
            command(r#"echo second-tail >> "$1/events""#),
        ],
        on_unlock: vec![
            command(r#"echo unlock-one >> "$1/events"; exit 3"#),
            command(r#"echo unlock-two >> "$1/events""#),
        ],
        ..IdleConfig::default()
    };
    let second_active = Arc::clone(&active);
    let second_retry = Arc::clone(&retry);
    let second_worker = thread::spawn(move || run_lock_cycle(second, PathBuf::new(), second_active, second_retry));
    wait(&|| !active.load(Ordering::Acquire));
    assert_eq!(events(), "first-start\nsecond-start\n");
    fs::write(directory.join("release-first"), "").unwrap();
    first_worker.join().unwrap();
    assert_eq!(events(), "first-start\nsecond-start\nfirst-end\nfirst-tail\n");
    fs::write(directory.join("release-second"), "").unwrap();
    second_worker.join().unwrap();
    let actual = events();
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(actual, "first-start\nsecond-start\nfirst-end\nfirst-tail\nsecond-failure\nsecond-tail\nunlock-one\nunlock-two\n");
    assert!(!active.load(Ordering::Acquire));
    assert!(!take_retry_request(&active, &retry));
}

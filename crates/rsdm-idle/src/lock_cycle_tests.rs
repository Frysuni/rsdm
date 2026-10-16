use super::*;

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

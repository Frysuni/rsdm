use std::os::unix::fs::symlink;

use super::*;

struct Records {
    directory: PathBuf,
    runtime: Runtime,
    record: SessionRecord,
}

impl Records {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-retirement-test-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let mut record = crate::helper_start_fixture::record();
        // SAFETY: geteuid has no preconditions.
        record.identity.uid = unsafe { libc::geteuid() };
        let runtime = Runtime::create_at(&directory, &record.identity.generation).unwrap();
        runtime.save_session(&record).unwrap();
        Self { directory, runtime, record }
    }
}

#[tokio::test]
async fn retirement_interrupts_a_pending_hook_even_if_its_loader_missed_the_transition() {
    let records = Records::new();
    let actions = crate::action_gate::ActionGate::default().with_generation(records.runtime.clone());
    let clone = actions.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let operation = tokio::spawn(async move {
        clone.guard(async { let _ = started.send(()); std::future::pending::<()>().await }).await
    });
    ready.await.unwrap();
    mark(&records.runtime).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), operation).await.unwrap().unwrap();
    assert!(matches!(result, Err(SessionError::Disabled)));
    assert!(matches!(actions.check(), Err(SessionError::Disabled)));
}

#[tokio::test]
async fn a_retired_generation_never_polls_a_new_hook_action() {
    let records = Records::new();
    let actions = crate::action_gate::ActionGate::default().with_generation(records.runtime.clone());
    mark(&records.runtime).unwrap();
    let polled = std::sync::atomic::AtomicBool::new(false);
    let result = actions.guard(async { polled.store(true, std::sync::atomic::Ordering::SeqCst); }).await;
    assert!(matches!(result, Err(SessionError::Disabled)));
    assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn configuration_disable_interrupts_a_guard_waiting_for_the_filesystem_pool() {
    let records = Records::new();
    let executor = tokio::runtime::Builder::new_current_thread().enable_all().max_blocking_threads(1).build().unwrap();
    executor.block_on(async {
        let (release, held) = std::sync::mpsc::channel();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || { entered.send(()).unwrap(); held.recv().unwrap(); });
        ready.await.unwrap();
        let (updates, receiver) = tokio::sync::watch::channel(rsdm_core::domain::SessionManagerUpdate::from(
            rsdm_core::domain::SessionManagerConfig::default(),
        ));
        let actions = crate::action_gate::ActionGate::new(Some(receiver)).with_generation(records.runtime.clone());
        let operation = tokio::spawn(async move { actions.guard(std::future::pending::<()>()).await });
        tokio::task::yield_now().await;
        updates.send_modify(|update| { update.config.enabled = false; update.disable_count += 1; });
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), operation).await;
        // Release the worker before asserting, including on a failed timeout.
        release.send(()).unwrap();
        blocker.await.unwrap();
        assert!(matches!(result.unwrap().unwrap(), Err(SessionError::Disabled)));
    });
}

impl Drop for Records {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.runtime.path, fs::Permissions::from_mode(0o700));
        let _ = fs::set_permissions(self.runtime.path.parent().unwrap(), fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn a_failed_record_replacement_permanently_retires_the_generation() {
    let mut records = Records::new();
    let path = records.runtime.path.join("session.toml");
    let saved = fs::read(&path).unwrap();
    assert!(!records.runtime.is_disabled().unwrap());
    fs::set_permissions(&records.runtime.path, fs::Permissions::from_mode(0o500)).unwrap();
    fs::set_permissions(records.runtime.path.parent().unwrap(), fs::Permissions::from_mode(0o500)).unwrap();
    records.record.disabled = true;
    assert!(records.runtime.save_session(&records.record).is_err());
    assert_eq!(fs::read(&path).unwrap(), saved);
    assert!(records.runtime.is_disabled().unwrap());
    assert!(records.runtime.session().unwrap().disabled);

    fs::set_permissions(&records.runtime.path, fs::Permissions::from_mode(0o700)).unwrap();
    records.record.disabled = false;
    assert!(records.runtime.save_session(&records.record).is_err());
    assert_eq!(fs::read(path).unwrap(), saved);
}

#[test]
fn retirement_is_checked_before_parsing_a_damaged_or_missing_session_record() {
    let mut records = Records::new();
    records.record.disabled = true;
    records.runtime.save_session(&records.record).unwrap();
    let path = records.runtime.path.join("session.toml");
    fs::write(&path, "invalid TOML").unwrap();
    assert!(records.runtime.is_disabled().unwrap());
    fs::remove_file(path).unwrap();
    assert!(records.runtime.is_disabled().unwrap());
    records.record.disabled = false;
    assert!(records.runtime.save_session(&records.record).is_err());
}

#[test]
fn recreating_a_directory_cannot_reset_a_retired_generation() {
    let mut records = Records::new();
    records.record.disabled = true;
    records.runtime.save_session(&records.record).unwrap();
    fs::remove_dir_all(&records.runtime.path).unwrap();
    let recreated = Runtime::create_at(&records.directory, &records.record.identity.generation).unwrap();
    assert!(recreated.is_disabled().unwrap());
    records.record.disabled = false;
    assert!(recreated.save_session(&records.record).is_err());
}

#[test]
fn legacy_records_use_their_disabled_field_without_a_retirement_slot() {
    let records = Records::new();
    fs::remove_file(records.runtime.path.with_extension("disabled")).unwrap();
    assert!(!records.runtime.is_disabled().unwrap());
    let mut record = records.record.clone();
    record.disabled = true;
    fs::write(records.runtime.path.join("session.toml"), toml::to_string(&record).unwrap()).unwrap();
    assert!(records.runtime.is_disabled().unwrap());
    assert!(records.runtime.save_session(&records.record).is_err());
}

#[test]
fn unsafe_or_partial_retirement_slots_refuse_reads_without_altering_foreign_data() {
    let records = Records::new();
    let path = records.runtime.path.with_extension("disabled");
    for content in [vec![], vec![2], vec![0, 1]] {
        fs::write(&path, &content).unwrap();
        assert!(records.runtime.is_disabled().is_err());
        assert!(mark(&records.runtime).is_err());
        assert_eq!(fs::read(&path).unwrap(), content);
    }
    fs::write(&path, [0]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(records.runtime.is_disabled().is_err());
    assert!(mark(&records.runtime).is_err());
    fs::remove_file(&path).unwrap();
    let foreign = records.directory.join("foreign");
    fs::write(&foreign, [0]).unwrap();
    symlink(&foreign, &path).unwrap();
    assert!(records.runtime.is_disabled().is_err());
    assert!(mark(&records.runtime).is_err());
    assert_eq!(fs::read(foreign).unwrap(), [0]);
}

use super::*;

#[tokio::test]
async fn disabling_persists_the_marker_and_rejects_pending_work_without_teardown() {
    let mut fixture = Fixture::new().await;
    let coordinator = &mut fixture.coordinator;
    let (sender, mut finalized) = tokio::sync::mpsc::channel(1);
    coordinator.finalize_replies.push(crate::control::Reply::for_test(sender));
    let (sender, mut stopped) = tokio::sync::mpsc::channel(1);
    coordinator.stop_replies.push(crate::control::Reply::for_test(sender));
    coordinator.preparing = true;
    coordinator.manager.deadline.set(1);

    tokio::time::timeout(Duration::from_secs(3), coordinator.disable()).await.unwrap().unwrap();
    assert!(coordinator.runtime.session().unwrap().disabled);
    assert!(coordinator.manager.actions.check().is_err());
    assert!(finalized.recv().await.unwrap().unwrap_err().contains("disabled"));
    assert_eq!(stopped.recv().await.unwrap().unwrap().result, "unknown");
    // Infrastructure detachment does not transition into session teardown.
    assert_eq!(coordinator.lifecycle.phase, SessionPhase::Running);
}

#[tokio::test]
async fn late_worker_completions_cannot_resume_preparation_after_disable() {
    let mut fixture = Fixture::new().await;
    let coordinator = &mut fixture.coordinator;
    coordinator.disable().await.unwrap();
    coordinator.workers = 3;
    coordinator.completed(Work::Prepared(Ok(vec!["forced.service".into()]))).await.unwrap();
    coordinator.completed(Work::Delegated(Ok(()))).await.unwrap();
    coordinator.completed(Work::EnvironmentPublished(Ok(()))).await.unwrap();
    assert_eq!(coordinator.workers, 0);
    assert!(coordinator.forced_units.is_empty());
    assert_eq!(coordinator.lifecycle.phase, SessionPhase::Running);
    assert!(coordinator.runtime.session().unwrap().disabled);
}

#[tokio::test]
async fn a_record_error_still_drains_workers_and_releases_pending_callers() {
    let mut fixture = Fixture::new().await;
    let coordinator = &mut fixture.coordinator;
    // A directory in place of the record forces atomic publication to fail.
    // The retirement slot must protect hooks even when TOML cannot be loaded.
    let path = coordinator.runtime.path.join("session.toml");
    fs::remove_file(&path).unwrap();
    fs::create_dir(path).unwrap();
    let (sender, mut finalized) = tokio::sync::mpsc::channel(1);
    coordinator.finalize_replies.push(crate::control::Reply::for_test(sender));
    coordinator.workers = 1;
    coordinator.events.send(Work::EnvironmentPublished(Ok(()))).unwrap();

    assert!(tokio::time::timeout(Duration::from_secs(3), coordinator.disable()).await.unwrap().is_err());
    assert_eq!(coordinator.workers, 0);
    assert!(coordinator.runtime.is_disabled().unwrap());
    assert!(coordinator.manager.actions.check().is_err());
    assert!(finalized.recv().await.unwrap().is_err());
    assert_eq!(coordinator.lifecycle.phase, SessionPhase::Running);
}

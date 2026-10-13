//! Failed recovery keeps the original launcher lifetime separate from unit ownership.

use super::*;

#[tokio::test]
async fn recovery_retains_the_launcher_after_losing_compositor_ownership() {
    for case in ["replaced", "unknown", "unverified"] {
        let current = if case == "replaced" { vec![2; 16] } else { vec![1; 16] };
        let (mut fixture, state) = fixture(vec![1; 16], current).await;
        if case == "unknown" { state.failed_query.store(true, Ordering::SeqCst); }
        let coordinator = &mut fixture.coordinator;
        if case == "unverified" { coordinator.record.compositor_invocation.clear(); }
        let mut process = SessionProcess::launch(&["unused".into()], &[], &fixture.directory,
            &coordinator.provider, "compositor.service".into()).unwrap();
        process.invocation = coordinator.record.compositor_invocation.clone();
        process.submitted.store(true, Ordering::Release);
        process.booting = false;
        coordinator.process = Some(process);
        coordinator.recover().await;
        assert!(coordinator.retain_launcher, "{case}");
        assert!(coordinator.process.is_some());
        assert!(state.stops.lock().unwrap().is_empty());
        assert!(state.mutations.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn recovery_retains_the_start_calls_process_when_a_worker_has_not_returned_it() {
    let (mut fixture, state) = fixture(vec![1; 16], vec![1; 16]).await;
    fixture.coordinator.manager.deadline.set(1);
    fixture.coordinator.workers = 1;
    fixture.coordinator.recover().await;
    assert!(fixture.coordinator.retain_launcher);
    assert!(fixture.coordinator.process.is_none());
    assert_eq!(fixture.coordinator.workers, 1);
    assert!(state.stops.lock().unwrap().is_empty());
    assert!(state.mutations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn passive_wait_does_not_finish_when_the_unit_is_reused_and_the_original_wm_is_alive() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (fixture, state) = fixture(vec![1; 16], vec![2; 16]).await;
    let mut child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "printf 'ready\\n'; read line; exit 7"])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut ready).await.unwrap();
    assert_eq!(ready, "ready\n");
    let coordinator = &fixture.coordinator;
    let mut process = SessionProcess::launch(&["unused".into()], &[], &fixture.directory,
        &coordinator.provider, "compositor.service".into()).unwrap();
    process.booting = false;
    process.submitted.store(true, Ordering::Release);
    *process.compositor.lock().unwrap() = Some(Arc::new(crate::processes::ProcessHandle::open(child.id().unwrap()).unwrap().unwrap()));
    let mut waiting = Box::pin(crate::session_wait::wait(process, &coordinator.record,
        &coordinator.provider, coordinator.manager.connection()));
    assert!(tokio::time::timeout(Duration::from_millis(50), &mut waiting).await.is_err());
    input.write_all(b"exit\n").await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(1), waiting).await.unwrap().is_err());
    assert_eq!(child.wait().await.unwrap().code(), Some(7));
    assert!(state.stops.lock().unwrap().is_empty());
    assert!(state.mutations.lock().unwrap().is_empty());
}

//! Live libSM traffic waits for the same four slots as user-manager requests.

use super::*;

type Requests = tokio::task::JoinSet<zbus::Result<()>>;

pub(super) async fn worker(mode: &str) {
    let fixture = Fixture::new().await;
    let provider = Provider { kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new() };
    if mode == "quota-startup" { startup(&fixture, &provider).await; return; }
    let mut handle = Handle::start(&fixture.manager, &fixture.runtime, &provider).await.unwrap();
    let client_mode = match mode { "quota-cancel" => "after-send", "quota-deadline" => "quota-disabled", _ => mode };
    let (mut child, mut output) = start_client(&fixture, &handle, client_mode).await;

    if mode == "quota-local" {
        blocked_reply(&fixture, &mut child, &mut output, "xsmp-client-local-requested", "xsmp-client-local-saving").await;
        blocked_reply(&fixture, &mut child, &mut output, "xsmp-client-local-done", "xsmp-client-local-complete").await;
    } else { shutdown(&fixture, &mut handle, &mut child, &mut output, mode).await; }
    assert!(tokio::time::timeout(Duration::from_secs(3), child.wait()).await.unwrap().unwrap().success());
    handle.shutdown(&Deadline::default()).await.unwrap();
}

async fn hold_slots(manager: &UserManager) -> (Requests, Arc<tokio::sync::Semaphore>) {
    let replies = Arc::new(tokio::sync::Semaphore::new(0));
    let (entered, mut slots) = mpsc::unbounded_channel();
    let mut workers = Requests::new();
    for _ in 0..4 {
        let manager = manager.clone();
        let entered = entered.clone();
        let replies = replies.clone();
        workers.spawn(async move {
            manager.request(async {
                entered.send(()).unwrap();
                replies.acquire().await.unwrap().forget();
                Ok(())
            }).await
        });
    }
    for _ in 0..4 { tokio::time::timeout(Duration::from_secs(2), slots.recv()).await.unwrap().unwrap(); }
    (workers, replies)
}

async fn release_slots(mut workers: Requests, replies: &tokio::sync::Semaphore) -> Vec<zbus::Result<()>> {
    replies.add_permits(4);
    let mut results = Vec::new();
    while let Some(result) = workers.join_next().await { results.push(result.unwrap()); }
    results
}

async fn startup(fixture: &Fixture, provider: &Provider) {
    let (workers, replies) = hold_slots(&fixture.manager).await;
    let mut pending = Box::pin(Handle::start(&fixture.manager, &fixture.runtime, provider));
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(35)) => {},
        _ = &mut pending => panic!("XSMP startup bypassed occupied request slots"),
    }
    assert!(!fixture.runtime.path.join("ICEauthority").exists());
    assert!(release_slots(workers, &replies).await.iter().all(Result::is_ok));
    let mut handle = pending.await.unwrap();
    handle.shutdown(&Deadline::default()).await.unwrap();
}

async fn blocked_reply(fixture: &Fixture, child: &mut tokio::process::Child,
    output: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>, request: &str, reply: &str) {
    let (workers, replies) = hold_slots(&fixture.manager).await;
    child.stdin.as_mut().unwrap().write_all(&[1]).await.unwrap();
    wait_marker(output, request).await;
    assert!(tokio::time::timeout(Duration::from_millis(35), wait_marker(output, reply)).await.is_err(),
        "{reply} bypassed occupied request slots");
    assert!(release_slots(workers, &replies).await.iter().all(Result::is_ok));
    wait_marker(output, reply).await;
}

async fn shutdown(fixture: &Fixture, handle: &mut Handle, child: &mut tokio::process::Child,
    output: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>, mode: &str) {
    let (reply, mut received) = mpsc::channel(1);
    handle.prepare(vec![crate::helper_start_fixture::app().unit], String::new(), true,
        Some(fixture.state.control.clone()), Reply::for_test(reply));
    received.recv().await.unwrap().unwrap();
    wait_marker(output, "xsmp-client-saving").await;
    let (mut workers, replies) = hold_slots(&fixture.manager).await;
    match mode {
        "quota-cancel" => {
            fixture.state.control.cancelled.store(true, Ordering::SeqCst);
            child.stdin.take().unwrap().write_all(&[1]).await.unwrap();
            wait_marker(output, "xsmp-client-followup-requested").await;
            assert!(tokio::time::timeout(Duration::from_millis(35), wait_marker(output, "xsmp-client-cancelled")).await.is_err());
        }
        "quota-disabled" => { fixture.manager.actions.close(); }
        "quota-deadline" => {
            fixture.state.control.cancelled.store(true, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(35)).await;
            fixture.state.deadline.set(1);
        }
        "quota-shutdown" => {
            tokio::time::timeout(Duration::from_millis(500), handle.shutdown(&Deadline::default())).await.unwrap().unwrap();
        }
        _ => panic!("unknown shutdown quota case: {mode}"),
    }
    let results = if mode == "quota-deadline" {
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut results = Vec::new();
            while let Some(result) = workers.join_next().await { results.push(result.unwrap()); }
            results
        }).await.unwrap()
    } else { release_slots(workers, &replies).await };
    assert!(results.iter().all(|result| if matches!(mode, "quota-disabled" | "quota-deadline") { result.is_err() } else { result.is_ok() }));
    if mode != "quota-shutdown" { wait_marker(output, "xsmp-client-cancelled").await; }
    handle.shutdown(&Deadline::default()).await.unwrap();
}

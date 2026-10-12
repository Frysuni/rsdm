//! Revoke ownership while a real libSM client is saving, before its next request.

use super::*;

pub(super) async fn worker(mode: &str) {
    let fixture = Fixture::new().await;
    if mode.ends_with("-lease") {
        let mut app = fixture.runtime.app(&crate::helper_start_fixture::app().unit).unwrap();
        app.policy.on_timeout = rsdm_core::domain::TimeoutAction::Cancel;
        fixture.runtime.save_app(&app).unwrap();
    }
    let provider = Provider { kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new() };
    let mut handle = Handle::start(&fixture.manager, &fixture.runtime, &provider).await.unwrap();
    let (mut child, mut output) = start_client(&fixture, &handle, mode).await;
    fixture.state.checks.store(0, Ordering::SeqCst);
    if mode.ends_with("-forced") { fixture.state.control.force(0); }

    let unit = crate::helper_start_fixture::app().unit;
    let (reply, mut received) = mpsc::channel(1);
    handle.prepare(vec![unit.clone()], String::new(), true, Some(fixture.state.control.clone()), Reply::for_test(reply));
    received.recv().await.unwrap().unwrap();
    wait_marker(&mut output, "xsmp-client-saving").await;
    release_client(&fixture, &mut child, &mut output, mode).await;

    if !mode.ends_with("-owned") {
        wait_marker(&mut output, "xsmp-client-cancelled").await;
        handle.shutdown(&Deadline::default()).await.unwrap();
    }
    assert!(tokio::time::timeout(Duration::from_secs(3), child.wait()).await.unwrap().unwrap().success(),
        "libSM client observed an unauthorized followup message");
    handle.shutdown(&Deadline::default()).await.unwrap();

    let app = fixture.runtime.app(&unit).unwrap();
    assert_eq!(app.invocation_id, vec![if mode.contains("-record") { 2 } else { 1 }; 16]);
    let dispatch = app.dispatch.as_ref().unwrap();
    assert_eq!(dispatch.state, DispatchState::Sent);
    if mode.ends_with("-owned") {
        assert_eq!(dispatch.xsmp_die_state, Some(DispatchState::Sent));
        crate::apps::reset_preparation(&fixture.runtime).await.unwrap();
        assert!(!crate::app_dispatch::claim(&fixture.runtime, &app, Some("next"), crate::app_dispatch::Method::Xsmp, &Deadline::default()).await.unwrap());
    }
    if mode.ends_with("-lease") { assert_eq!(dispatch.xsmp_die_state, Some(DispatchState::NotSent)); }
}

async fn release_client(fixture: &Fixture, child: &mut tokio::process::Child,
    output: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>, mode: &str) {
    if mode.ends_with("-lease") { shorten_during_claim(fixture, child, output).await; return; }
    if !mode.ends_with("-slots") {
        revoke(fixture, mode);
        child.stdin.take().unwrap().write_all(&[1]).await.unwrap();
        return;
    }

    let replies = Arc::new(tokio::sync::Semaphore::new(0));
    let (entered, mut slots) = mpsc::unbounded_channel();
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let manager = fixture.manager.clone();
        let entered = entered.clone();
        let replies = replies.clone();
        workers.spawn(async move {
            manager.request(async {
                entered.send(()).unwrap();
                replies.acquire().await.unwrap().forget();
                Ok(())
            }).await.unwrap();
        });
    }
    for _ in 0..4 { tokio::time::timeout(Duration::from_secs(2), slots.recv()).await.unwrap().unwrap(); }
    child.stdin.take().unwrap().write_all(&[1]).await.unwrap();
    wait_marker(output, "xsmp-client-followup-requested").await;
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(fixture.state.checks.load(Ordering::SeqCst), 3, "ownership query bypassed occupied IPC slots");
    fixture.state.invocation.store(2, Ordering::SeqCst);
    replies.add_permits(4);
    while let Some(result) = workers.join_next().await { result.unwrap(); }
}

async fn shorten_during_claim(fixture: &Fixture, child: &mut tokio::process::Child,
    output: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>) {
    let lease = fixture.runtime.app_lease(&crate::helper_start_fixture::app().unit).unwrap();
    child.stdin.take().unwrap().write_all(&[1]).await.unwrap();
    wait_marker(output, "xsmp-client-followup-requested").await;
    tokio::time::sleep(Duration::from_millis(25)).await;
    let mut session = fixture.runtime.session().unwrap();
    session.shutdown_deadline_usec = Some(1);
    fixture.runtime.save_session(&session).unwrap();
    drop(lease);
}
fn revoke(fixture: &Fixture, mode: &str) {
    let change = mode.split('-').nth(2).unwrap();
    match change {
        "owned" => {},
        "invocation" => { fixture.state.invocation.store(2, Ordering::SeqCst); }
        "deadline" => { fixture.state.expire_final.store(true, Ordering::SeqCst); }
        "cancel" => { fixture.state.cancel_final.store(true, Ordering::SeqCst); }
        "cgroup" => { fixture.state.process_present.store(false, Ordering::SeqCst); }
        "control" => { fixture.state.control_pid.store(fixture.state.pid.load(Ordering::SeqCst), Ordering::SeqCst); }
        "record" => {
            let mut app = fixture.runtime.app(&crate::helper_start_fixture::app().unit).unwrap();
            app.invocation_id = vec![2; 16];
            fixture.runtime.save_app(&app).unwrap();
        }
        "transaction" | "disabled" => {
            let mut session = fixture.runtime.session().unwrap();
            if change == "disabled" { session.disabled = true; }
            else {
                session.shutdown_transaction = Some(crate::shutdown_transaction::ShutdownTransaction {
                    id: "replacement".into(), action: "logout".into(), started_usec: 1,
                    deadline_usec: i64::MAX as u64, result: None,
                });
            }
            fixture.runtime.save_session(&session).unwrap();
        }
        _ => panic!("unknown ownership change: {change}"),
    }
}

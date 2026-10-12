//! All admitted clients in one application's current save receive Die.

use super::*;

pub(super) async fn worker() {
    let fixture = Fixture::new().await;
    let provider = Provider { kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new() };
    let mut handle = Handle::start(&fixture.manager, &fixture.runtime, &provider).await.unwrap();
    let (mut first, mut first_output) = start_client(&fixture, &handle, "followup-die-owned").await;
    let (mut second, mut second_output) = start_client(&fixture, &handle, "followup-die-owned").await;

    let unit = crate::helper_start_fixture::app().unit;
    let (reply, mut received) = mpsc::channel(1);
    handle.prepare(vec![unit.clone()], String::new(), true, Some(fixture.state.control.clone()), Reply::for_test(reply));
    received.recv().await.unwrap().unwrap();
    wait_marker(&mut first_output, "xsmp-client-saving").await;
    wait_marker(&mut second_output, "xsmp-client-saving").await;
    first.stdin.take().unwrap().write_all(&[1]).await.unwrap();
    second.stdin.take().unwrap().write_all(&[1]).await.unwrap();

    for child in [&mut first, &mut second] {
        assert!(tokio::time::timeout(Duration::from_secs(3), child.wait()).await.unwrap().unwrap().success());
    }
    handle.shutdown(&Deadline::default()).await.unwrap();
    let app = fixture.runtime.app(&unit).unwrap();
    assert_eq!(app.dispatch.as_ref().unwrap().xsmp_die_state, Some(DispatchState::Sent));
    crate::apps::reset_preparation(&fixture.runtime).await.unwrap();
    assert!(!crate::app_dispatch::claim(&fixture.runtime, &app, Some("next"), crate::app_dispatch::Method::Xsmp, &Deadline::default()).await.unwrap());
}

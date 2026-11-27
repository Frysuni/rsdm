//! Application batches preserve protocol selection and invocation ownership.

use super::*;

#[derive(Default, Clone)]
struct Selection(Arc<Mutex<Vec<Vec<String>>>>);

#[zbus_macros::interface(name = "org.rsdm.Session1")]
impl Selection {
    fn xsmp_prepare(&self, generation: &str, _transaction: &str, units: Vec<String>, _cancelable: bool) -> Vec<String> {
        assert_eq!(generation, GENERATION);
        self.0.lock().unwrap().push(units.clone());
        units
    }
}

#[tokio::test]
async fn later_batches_observe_confirmed_xsmp_instead_of_sending_a_second_close() {
    let fixture = Fixture::new().await;
    fixture._server.object_server().at(crate::control::OBJECT_PATH, Selection::default()).await.unwrap();
    let mut later = fixture.app.clone();
    later.unit = later.unit.replace("example", "later");
    fixture.runtime.save_app(&later).unwrap();
    let control = ShutdownControl::default();
    begin_batch(&fixture.manager, &fixture.runtime, std::slice::from_ref(&fixture.app), &control).await.unwrap();
    begin_batch(&fixture.manager, &fixture.runtime, std::slice::from_ref(&later), &control).await.unwrap();
    for app in [&fixture.app, &later] {
        request_quit(&fixture.manager, &fixture.runtime, app, &control).await.unwrap();
        let saved = fixture.runtime.app(&app.unit).unwrap();
        assert_eq!(saved.dispatch.unwrap().method, crate::app_dispatch::Method::Xsmp);
        assert!(saved.quit_started);
        assert!(!saved.window_close_sent);
    }
    assert_eq!(fixture.queries.load(Ordering::SeqCst), 0);
}


#[tokio::test]
async fn stale_invocation_aborts_the_whole_batch_before_dispatch_and_allows_a_fresh_retry() {
    let fixture = Fixture::new().await;
    let selection = Selection::default();
    fixture._server.object_server().at(crate::control::OBJECT_PATH, selection.clone()).await.unwrap();
    let mut stale = fixture.app.clone();
    stale.unit = stale.unit.replace("example", "replaced");
    fixture.runtime.save_app(&stale).unwrap();
    let mut replacement = stale.clone();
    replacement.invocation_id = vec![2; 16];
    fixture.runtime.save_app(&replacement).unwrap();

    let failed = Arc::new(ShutdownControl { transaction_id: Some("first".into()), ..ShutdownControl::default() });
    let result = prepare_apps(&fixture.manager, &fixture.runtime, vec![fixture.app.clone(), stale.clone()], failed.clone()).await;
    assert!(matches!(result, Err(SessionError::State(message)) if message.contains("invocation changed")));
    assert!(selection.0.lock().unwrap().is_empty());
    assert_eq!(fixture.queries.load(Ordering::SeqCst), 0);
    let first = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert!(first.dispatch.is_none());
    assert_eq!(first.preparation.unwrap().outcome, AppOutcome::Unknown);
    let saved_replacement = fixture.runtime.app(&replacement.unit).unwrap();
    assert_eq!(saved_replacement.invocation_id, replacement.invocation_id);
    assert!(saved_replacement.preparation.is_none());
    assert!(saved_replacement.dispatch.is_none());

    let next = ShutdownControl { transaction_id: Some("next".into()), ..ShutdownControl::default() };
    begin_batch(&fixture.manager, &fixture.runtime, &[fixture.app.clone(), replacement.clone()], &next).await.unwrap();
    assert_eq!(*selection.0.lock().unwrap(), [vec![fixture.app.unit.clone(), replacement.unit.clone()]]);
    for app in [&fixture.app, &replacement] {
        request_quit(&fixture.manager, &fixture.runtime, app, &next).await.unwrap();
        let saved = fixture.runtime.app(&app.unit).unwrap();
        let dispatch = saved.dispatch.unwrap();
        assert_eq!(dispatch.invocation_id, app.invocation_id);
        assert_eq!(dispatch.transaction_id.as_deref(), Some("next"));
        assert_eq!(dispatch.method, crate::app_dispatch::Method::Xsmp);
        assert_eq!(saved.preparation.unwrap().transaction_id.as_deref(), Some("next"));
        assert!(!saved.window_close_sent);
    }
    assert_eq!(fixture.queries.load(Ordering::SeqCst), 0);
}

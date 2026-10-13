//! A later native remainder keeps XSMP selection for both application batches.

use super::*;

struct Selection;

#[zbus_macros::interface(name = "org.rsdm.Session1")]
impl Selection {
    fn xsmp_prepare(&self, generation: &str, _transaction: &str, units: Vec<String>, _cancelable: bool) -> Vec<String> {
        assert_eq!(generation, GENERATION);
        units
    }
}

#[tokio::test]
async fn later_batches_observe_confirmed_xsmp_instead_of_sending_a_second_close() {
    let fixture = Fixture::new().await;
    fixture._server.object_server().at(crate::control::OBJECT_PATH, Selection).await.unwrap();
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

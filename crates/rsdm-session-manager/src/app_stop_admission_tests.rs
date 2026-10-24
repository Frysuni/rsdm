//! Only explicit pre-dispatch refusals may repeat an XSMP prepare request.

use super::*;

#[derive(Clone, Copy)]
enum Failure { NoReply, Denied, OtherLimit }

struct Selection {
    calls: Arc<AtomicUsize>,
    refusals: usize,
    refusal: &'static str,
    failure: Option<Failure>,
}

#[zbus_macros::interface(name = "org.rsdm.Session1")]
impl Selection {
    fn xsmp_prepare(&self, generation: &str, _transaction: &str, units: Vec<String>, _cancelable: bool) -> zbus::fdo::Result<Vec<String>> {
        assert_eq!(generation, GENERATION);
        if self.calls.fetch_add(1, Ordering::SeqCst) < self.refusals {
            return Err(zbus::fdo::Error::LimitsExceeded(self.refusal.into()));
        }
        match self.failure {
            Some(Failure::NoReply) => Err(zbus::fdo::Error::NoReply("accepted save lost its reply".into())),
            Some(Failure::Denied) => Err(zbus::fdo::Error::AccessDenied("not authorized".into())),
            Some(Failure::OtherLimit) => Err(zbus::fdo::Error::LimitsExceeded("save protocol limit".into())),
            None => Ok(units),
        }
    }
}

async fn install(fixture: &Fixture, refusals: usize, refusal: &'static str, failure: Option<Failure>) -> Arc<AtomicUsize> {
    let calls = Arc::new(AtomicUsize::new(0));
    fixture._server.object_server().at(crate::control::OBJECT_PATH,
        Selection { calls: calls.clone(), refusals, refusal, failure }).await.unwrap();
    calls
}

#[tokio::test]
async fn refused_xsmp_admission_retries_until_one_prepare_is_accepted() {
    for refusal in ["session coordinator is busy; retry the request", "session authorization is busy; retry the request", "session coordinator queue is full"] {
        let fixture = Fixture::new().await;
        let calls = install(&fixture, 2, refusal, None).await;
        fixture.manager.deadline.set(monotonic_usec().unwrap() + 1_000_000);
        let units = vec![fixture.app.unit.clone()];
        assert_eq!(prepare_xsmp(&fixture.manager, &fixture.runtime, &units, &ShutdownControl::default()).await.unwrap(), units);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
}

#[tokio::test]
async fn accepted_or_unrelated_xsmp_errors_are_never_replayed() {
    for failure in [Failure::NoReply, Failure::Denied, Failure::OtherLimit] {
        let fixture = Fixture::new().await;
        let calls = install(&fixture, 0, "", Some(failure)).await;
        assert!(prepare_xsmp(&fixture.manager, &fixture.runtime, &[fixture.app.unit.clone()], &ShutdownControl::default()).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn busy_xsmp_admission_keeps_the_existing_shutdown_deadline() {
    let fixture = Fixture::new().await;
    install(&fixture, usize::MAX, "session coordinator is busy; retry the request", None).await;
    fixture.manager.deadline.set(monotonic_usec().unwrap() + 100_000);
    let before = Instant::now();
    let result = prepare_xsmp(&fixture.manager, &fixture.runtime, &[fixture.app.unit.clone()], &ShutdownControl::default()).await;
    assert!(matches!(result, Err(SessionError::Bus(zbus::Error::FDO(error))) if matches!(*error, zbus::fdo::Error::TimedOut(_))));
    assert!(before.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn cancelling_busy_xsmp_admission_prevents_a_later_prepare() {
    let fixture = Fixture::new().await;
    let calls = install(&fixture, usize::MAX, "session coordinator is busy; retry the request", None).await;
    let control = ShutdownControl::default();
    let units = [fixture.app.unit.clone()];
    let (_, result) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(async {
            while calls.load(Ordering::SeqCst) == 0 { tokio::time::sleep(Duration::from_millis(1)).await; }
            control.cancelled.store(true, Ordering::SeqCst);
        }, prepare_xsmp(&fixture.manager, &fixture.runtime, &units, &control))
    }).await.unwrap();
    assert!(result.unwrap().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn busy_xsmp_admission_without_a_shutdown_deadline_keeps_the_request_budget() {
    let fixture = Fixture::new().await;
    install(&fixture, usize::MAX, "session coordinator is busy; retry the request", None).await;
    let units = [fixture.app.unit.clone()];
    let result = tokio::time::timeout(Duration::from_secs(7),
        prepare_xsmp(&fixture.manager, &fixture.runtime, &units, &ShutdownControl::default())).await.unwrap();
    assert!(matches!(result, Err(SessionError::Bus(zbus::Error::FDO(error))) if matches!(*error, zbus::fdo::Error::TimedOut(_))));
}

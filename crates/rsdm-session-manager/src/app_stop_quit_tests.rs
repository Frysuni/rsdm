use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc};

use zbus::zvariant::OwnedObjectPath;

use super::*;

struct Manager(Option<Arc<ShutdownControl>>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, unit: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        if let Some(control) = &self.0 {
            control.cancelled.store(true, Ordering::SeqCst);
        }
        if unit != crate::attach::UNIT { return Err(zbus::fdo::Error::Failed("unexpected unit lookup".into())); }
        Ok(OwnedObjectPath::try_from("/coordinator").unwrap())
    }
}

struct Unit;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> { vec![1; 16] }
}

struct Fixture { directory: PathBuf, runtime: Runtime, manager: UserManager, _server: zbus::Connection }

impl Fixture {
    async fn new(control: Option<Arc<ShutdownControl>>) -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-quit-anchor-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::create_at(&directory, crate::helper_start_fixture::GENERATION).unwrap();
        let (server, client) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server).p2p().server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager(control)).unwrap()
                .serve_at("/coordinator", Unit).unwrap().build().await.unwrap()
        });
        let client = zbus::connection::Builder::unix_stream(client).p2p().build().await.unwrap();
        Self { directory, runtime, manager: UserManager::with_connection(client), _server: server.await.unwrap() }
    }

    fn save(&self, provider: &str, coordinator: Option<(String, Vec<u8>)>) {
        let mut record = crate::helper_start_fixture::record();
        // SAFETY: geteuid has no preconditions.
        record.identity.uid = unsafe { libc::geteuid() };
        record.provider = provider.into();
        record.coordinator_unit = coordinator;
        self.runtime.save_session(&record).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

#[tokio::test]
async fn owned_sessions_use_the_anchor_and_attached_sessions_use_the_verified_coordinator() {
    let fixture = Fixture::new(None).await;
    fixture.save("managed", None);
    assert_eq!(quit_anchor(&fixture.manager, &fixture.runtime).await.unwrap(), crate::helper_start_fixture::record().anchor_unit);
    fixture.save("partial", Some((crate::attach::UNIT.into(), vec![1; 16])));
    assert_eq!(quit_anchor(&fixture.manager, &fixture.runtime).await.unwrap(), crate::attach::UNIT);
}

#[tokio::test]
async fn attached_sessions_reject_missing_foreign_or_replaced_coordinator_ownership() {
    let fixture = Fixture::new(None).await;
    for coordinator in [None, Some(("foreign.service".into(), vec![1; 16])),
        Some((crate::attach::UNIT.into(), Vec::new())), Some((crate::attach::UNIT.into(), vec![0; 16])),
        Some((crate::attach::UNIT.into(), vec![2; 16]))]
    {
        fixture.save("partial", coordinator);
        assert!(quit_anchor(&fixture.manager, &fixture.runtime).await.is_err());
    }
}

#[tokio::test]
async fn cancellation_during_window_discovery_prevents_term_fallback() {
    let control = Arc::new(ShutdownControl::default());
    control.xsmp_units.set(Vec::new()).unwrap();
    let fixture = Fixture::new(Some(control.clone())).await;
    fixture.save("managed", None);
    let socket = fixture.directory.join("sway.sock");
    let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let mut record = fixture.runtime.session().unwrap();
    record.compositor_unit = Some("sway.service".into());
    record.compositor_invocation = vec![1; 16];
    record.exported_environment = vec![("SWAYSOCK".into(), socket.to_str().unwrap().into())];
    fixture.runtime.save_session(&record).unwrap();
    let mut app = crate::helper_start_fixture::app();
    app.policy.quit_command.clear();
    fixture.runtime.save_app(&app).unwrap();

    assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap().is_none());
    assert!(control.cancelled());
    let saved = fixture.runtime.app(&app.unit).unwrap();
    assert!(!saved.quit_started);
    assert!(saved.dispatch.is_none());
}

#[tokio::test]
async fn cancellation_while_waiting_for_a_term_claim_leaves_it_unsent() {
    let fixture = Fixture::new(None).await;
    let mut app = crate::helper_start_fixture::app();
    app.policy.quit_command.clear();
    app.policy.method = ShutdownMethod::Term;
    fixture.runtime.save_app(&app).unwrap();
    let lease = fixture.runtime.app_lease(&app.unit).unwrap();
    let control = ShutdownControl::default();
    let mut request = Box::pin(request_quit(&fixture.manager, &fixture.runtime, &app, &control));

    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(20)) => {},
        result = &mut request => panic!("contended TERM claim completed: {result:?}"),
    }
    control.cancelled.store(true, Ordering::SeqCst);
    drop(lease);
    assert!(tokio::time::timeout(Duration::from_secs(2), request).await.unwrap().unwrap().is_none());
    let saved = fixture.runtime.app(&app.unit).unwrap();
    let dispatch = saved.dispatch.unwrap();
    assert_eq!(dispatch.method, crate::app_dispatch::Method::Term);
    assert_eq!(dispatch.state, crate::app_dispatch::State::NotSent);
}

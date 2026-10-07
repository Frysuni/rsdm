use std::{os::unix::net::UnixStream, sync::{Arc, Mutex}, thread};

use zbus::{object_server::SignalEmitter, zvariant::OwnedValue};

use super::*;

type Calls = Arc<Mutex<Vec<(String, String)>>>;

#[derive(Debug, zbus_macros::DBusError)]
#[zbus(prefix = "org.freedesktop.systemd1")]
enum ManagerError {
    NoSuchUnit(String),
    #[zbus(error)]
    Bus(zbus::Error),
}

struct FakeManager {
    version: &'static str,
    result: &'static str,
    calls: Calls,
}

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl FakeManager {
    #[zbus(property)]
    fn version(&self) -> &str {
        self.version
    }

    fn subscribe(&self) {}

    fn get_unit(&self, _unit: &str) -> Result<OwnedObjectPath, ManagerError> {
        Err(ManagerError::NoSuchUnit("not started".into()))
    }

    async fn start_transient_unit(
        &self,
        unit: &str,
        mode: &str,
        _properties: Vec<(String, OwnedValue)>,
        _auxiliary: Vec<(String, Vec<(String, OwnedValue)>)>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.calls.lock().unwrap().push((unit.into(), mode.into()));
        let other = OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/41").unwrap();
        Self::job_removed(&emitter, 41, &other, unit, "failed").await?;
        let job = OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/42").unwrap();
        Self::job_removed(&emitter, 42, &job, unit, self.result).await?;
        // Exercise completion before the method reply, not only after it.
        async_io::Timer::after(Duration::from_millis(20)).await;
        Ok(job)
    }

    #[zbus(signal)]
    async fn job_removed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        job: &OwnedObjectPath,
        unit: &str,
        result: &str,
    ) -> zbus::Result<()>;
}

fn connect(version: &'static str, result: &'static str) -> (UserManager, Connection, Calls) {
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let calls = Calls::default();
    let manager = FakeManager { version, result, calls: calls.clone() };
    let server = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server_socket)
            .p2p().server(zbus::Guid::generate()).unwrap()
            .serve_at(MANAGER_PATH, manager).unwrap().build().await.unwrap()
    }));
    let client = async_io::block_on(async {
        zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap()
    });
    let server = server.join().unwrap();
    let client = async_io::block_on(UserManager::from_connection(client)).unwrap();
    (client, server, calls)
}

#[test]
fn accepts_an_early_job_signal_and_ignores_another_job_for_the_same_unit() {
    let (manager, _server, calls) = connect("250", "done");
    manager.start_service("example.service", &Vec::new()).unwrap();
    assert_eq!(*calls.lock().unwrap(), [("example.service".into(), "fail".into())]);
}

#[test]
fn accepted_method_call_does_not_hide_a_failed_job() {
    let (manager, _server, _) = connect("260.1", "failed");
    let error = manager.start_service("example.service", &Vec::new()).unwrap_err();
    assert!(error.to_string().contains("job completed with failed"));
}

#[test]
fn job_wait_has_a_deadline_even_if_no_matching_signal_arrives() {
    let (manager, _server, _) = connect("250", "done");
    async_io::block_on(async {
        let proxy = manager.proxy().await.unwrap();
        let mut signals = proxy.receive_signal("JobRemoved").await.unwrap();
        let job = OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/999").unwrap();
        assert!(!wait_job(&mut signals, &job, Duration::from_millis(20)).await.unwrap());
    });
}

#[test]
fn transient_read_errors_are_retried_until_the_manager_returns() {
    let attempts = std::sync::atomic::AtomicUsize::new(0);
    let result = async_io::block_on(retry_read(|| async {
        if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
            Err(zbus::fdo::Error::NoReply("Remote peer disconnected".into()).into())
        } else {
            Ok("active")
        }
    }, Duration::from_secs(1))).unwrap();
    assert_eq!(result, "active");
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[test]
fn a_denied_read_is_not_retried() {
    let attempts = std::sync::atomic::AtomicUsize::new(0);
    let result: zbus::Result<()> = async_io::block_on(retry_read(|| async {
        attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(zbus::fdo::Error::AccessDenied("denied".into()).into())
    }, Duration::from_secs(1)));
    assert!(result.is_err());
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn retry_deadline_also_bounds_a_read_that_never_replies() {
    let before = Instant::now();
    let result: zbus::Result<()> = async_io::block_on(retry_read(
        || std::future::pending(), Duration::from_millis(30),
    ));
    assert!(result.is_err());
    assert!(before.elapsed() < Duration::from_secs(1));
}

#[test]
fn only_transport_and_reexecution_errors_are_retryable() {
    assert!(retryable_error(&zbus::fdo::Error::NoReply("disconnected".into()).into()));
    assert!(retryable_error(&zbus::fdo::Error::NameHasNoOwner("reexec".into()).into()));
    assert!(!retryable_error(&zbus::fdo::Error::InvalidArgs("invalid".into()).into()));
    assert!(!retryable_error(&zbus::fdo::Error::AccessDenied("denied".into()).into()));
    assert!(!retryable_error(&zbus::Error::Failure("failed".into())));
}

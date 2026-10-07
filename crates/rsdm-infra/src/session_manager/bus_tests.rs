use std::{os::unix::net::UnixStream, sync::{Arc, Mutex}, thread};

use zbus::{object_server::SignalEmitter, zvariant::OwnedValue};

use super::*;

type Calls = Arc<Mutex<Vec<(String, String)>>>;

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
        let error = wait_job(&mut signals, &job, Duration::from_millis(20)).await.unwrap_err();
        assert!(error.to_string().contains("timed out"));
    });
}

use std::{io::Write, os::unix::net::UnixStream, sync::{Arc, Mutex, mpsc}, time::Instant};

use zbus::{object_server::SignalEmitter, zvariant::OwnedFd as BusFd};

use super::*;

struct Logind {
    descriptor: Arc<Mutex<Option<OwnedFd>>>,
    preparing: bool,
}

#[zbus_macros::interface(name = "org.freedesktop.login1.Manager")]
impl Logind {
    fn inhibit(&self, what: &str, who: &str, _reason: &str, mode: &str) -> zbus::fdo::Result<BusFd> {
        assert_eq!((what, who, mode), ("shutdown", "RSDM", "delay"));
        self.descriptor.lock().unwrap().take().map(Into::into)
            .ok_or_else(|| zbus::fdo::Error::AccessDenied("inhibition denied".into()))
    }

    #[zbus(property, name = "InhibitDelayMaxUSec")]
    fn inhibit_delay_max_usec(&self) -> u64 { 5_000_000 }

    #[zbus(property)]
    fn preparing_for_shutdown(&self) -> bool { self.preparing }

    #[zbus(signal)]
    async fn prepare_for_shutdown(emitter: &SignalEmitter<'_>, preparing: bool) -> zbus::Result<()>;
}

fn connect(preparing: bool) -> (Connection, Connection, UnixStream, Arc<Mutex<Option<OwnedFd>>>) {
    let (guard, writer) = UnixStream::pair().unwrap();
    let descriptor = Arc::new(Mutex::new(Some(guard.into())));
    let service = Logind { descriptor: descriptor.clone(), preparing };
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server_socket).p2p()
            .server(zbus::Guid::generate()).unwrap().serve_at(PATH, service).unwrap().build().await.unwrap()
    }));
    let connection = async_io::block_on(async {
        zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap()
    });
    (connection, worker.join().unwrap(), writer, descriptor)
}

#[test]
fn inhibitor_descriptor_survives_the_bus_call_until_its_owner_is_dropped() {
    let (connection, _server, mut writer, _) = connect(false);
    let guard = async_io::block_on(inhibit(&connection, "test PAM lifetime")).unwrap();
    writer.write_all(b"held").unwrap();
    drop(guard);
    let deadline = Instant::now() + Duration::from_secs(2);
    while writer.write_all(b"released").is_ok() {
        assert!(Instant::now() < deadline, "inhibitor descriptor leaked");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn monitor_reports_initial_shutdown_and_both_signal_values_with_the_delay_budget() {
    let (connection, server, mut writer, _) = connect(true);
    let (notices, received) = mpsc::channel();
    let monitor = async_io::block_on(ShutdownMonitor::listen(connection, notices)).unwrap();
    let initial = received.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(initial.preparing);
    assert_eq!(initial.budget_usec, 4_750_000);
    for preparing in [false, true] {
        async_io::block_on(async {
            let emitter = SignalEmitter::new(&server, PATH).unwrap();
            Logind::prepare_for_shutdown(&emitter, preparing).await.unwrap();
        });
        let notice = received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(notice.preparing, preparing);
        assert_eq!(notice.budget_usec, 4_750_000);
    }
    writer.write_all(b"monitor owns the guard").unwrap();
    drop(monitor);
    assert!(writer.write_all(b"released").is_err());
}

#[test]
fn denied_user_inhibitor_keeps_shutdown_notifications_available() {
    let (connection, server, _writer, descriptor) = connect(false);
    drop(descriptor.lock().unwrap().take());
    let (notices, received) = mpsc::channel();
    let monitor = async_io::block_on(ShutdownMonitor::listen(connection, notices)).unwrap();
    assert!(monitor._inhibitor.is_none());
    async_io::block_on(async {
        let emitter = SignalEmitter::new(&server, PATH).unwrap();
        Logind::prepare_for_shutdown(&emitter, true).await.unwrap();
    });
    assert!(received.recv_timeout(Duration::from_secs(2)).unwrap().preparing);
}

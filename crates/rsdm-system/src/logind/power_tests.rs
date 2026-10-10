use std::{io::Write, os::unix::net::UnixStream, sync::{Arc, Mutex}, time::Instant};

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

async fn connect(preparing: bool) -> (Connection, Connection, UnixStream, Arc<Mutex<Option<OwnedFd>>>) {
    let (guard, writer) = UnixStream::pair().unwrap();
    let descriptor = Arc::new(Mutex::new(Some(guard.into())));
    let service = Logind { descriptor: descriptor.clone(), preparing };
    let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let worker = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket).p2p()
            .server(zbus::Guid::generate()).unwrap().serve_at(PATH, service).unwrap().build().await.unwrap()
    });
    let connection = zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap();
    (connection, worker.await.unwrap(), writer, descriptor)
}

#[tokio::test]
async fn inhibitor_descriptor_survives_the_bus_call_until_its_owner_is_dropped() {
    let (connection, _server, mut writer, _) = connect(false).await;
    let guard = inhibit(&connection, "test PAM lifetime").await.unwrap();
    writer.write_all(b"held").unwrap();
    drop(guard);
    let deadline = Instant::now() + Duration::from_secs(2);
    while writer.write_all(b"released").is_ok() {
        assert!(Instant::now() < deadline, "inhibitor descriptor leaked");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn monitor_reports_initial_shutdown_and_both_signal_values_with_the_delay_budget() {
    let (connection, server, mut writer, _) = connect(true).await;
    let (notices, mut received) = tokio::sync::mpsc::unbounded_channel();
    let before = crate::clock::monotonic_usec().unwrap();
    let monitor = ShutdownMonitor::listen(connection, notices, "test shutdown monitor").await.unwrap();
    let initial = tokio::time::timeout(Duration::from_secs(2), received.recv()).await.unwrap().unwrap();
    assert!(initial.preparing);
    let after = crate::clock::monotonic_usec().unwrap();
    assert!((before + 4_750_000..=after + 4_750_000).contains(&initial.deadline_usec));
    for preparing in [false, true] {
        let before = crate::clock::monotonic_usec().unwrap();
        let emitter = SignalEmitter::new(&server, PATH).unwrap();
        Logind::prepare_for_shutdown(&emitter, preparing).await.unwrap();
        let notice = tokio::time::timeout(Duration::from_secs(2), received.recv()).await.unwrap().unwrap();
        assert_eq!(notice.preparing, preparing);
        let after = crate::clock::monotonic_usec().unwrap();
        if preparing {
            assert!((before + 4_750_000..=after + 4_750_000).contains(&notice.deadline_usec));
        } else {
            assert_eq!(notice.deadline_usec, 0);
        }
    }
    writer.write_all(b"monitor owns the guard").unwrap();
    drop(monitor);
    assert!(writer.write_all(b"released").is_err());
    assert!(tokio::time::timeout(Duration::from_secs(2), received.recv()).await.unwrap().is_none());
}

#[tokio::test]
async fn denied_user_inhibitor_keeps_shutdown_notifications_available() {
    let (connection, server, _writer, descriptor) = connect(false).await;
    drop(descriptor.lock().unwrap().take());
    let (notices, mut received) = tokio::sync::mpsc::unbounded_channel();
    let monitor = ShutdownMonitor::listen(connection, notices, "test shutdown monitor").await.unwrap();
    assert!(monitor._inhibitor.is_none());
    {
        let emitter = SignalEmitter::new(&server, PATH).unwrap();
        Logind::prepare_for_shutdown(&emitter, true).await.unwrap();
    }
    assert!(tokio::time::timeout(Duration::from_secs(2), received.recv()).await.unwrap().unwrap().preparing);
}

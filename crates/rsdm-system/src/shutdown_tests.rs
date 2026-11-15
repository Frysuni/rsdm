use std::{sync::{Arc, Mutex}, time::Instant};

use super::*;

type Calls = Arc<Mutex<Vec<String>>>;
type WireJob = (u32, String, String, String, OwnedObjectPath, OwnedObjectPath);
type WireInhibitor = (String, String, String, String, u32, u32);

struct Manager { calls: Calls, hung: bool }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    async fn list_jobs(&self, #[zbus(header)] header: zbus::message::Header<'_>) -> Vec<WireJob> {
        assert!(header.primary().flags().contains(zbus::message::Flags::NO_AUTO_START));
        self.calls.lock().unwrap().push("ListJobs".into());
        if self.hung { std::future::pending::<()>().await; }
        vec![(17, "slow.service".into(), "stop".into(), "running".into(),
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/17").unwrap(),
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/slow").unwrap())]
    }
}

struct Login { calls: Calls }

#[zbus_macros::interface(name = "org.freedesktop.login1.Manager")]
impl Login {
    fn list_inhibitors(&self, #[zbus(header)] header: zbus::message::Header<'_>) -> Vec<WireInhibitor> {
        assert!(header.primary().flags().contains(zbus::message::Flags::NO_AUTO_START));
        self.calls.lock().unwrap().push("ListInhibitors".into());
        ["sleep", "shutdown:sleep", "shutdown", "handle-power-key"].into_iter().enumerate()
            .map(|(index, what)| (what.into(), "Editor".into(), "Saving files".into(), "delay".into(), 1000, index as u32 + 10)).collect()
    }

    #[zbus(property)]
    fn preparing_for_shutdown(&self) -> bool { true }
}

async fn fixture(hung: bool) -> (Connection, Connection, Calls) {
    let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let calls = Calls::default();
    let state = calls.clone();
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket.into_std().unwrap()).p2p().server(zbus::Guid::generate())
            .serve_at("/org/freedesktop/systemd1", Manager { calls: state.clone(), hung })
            .serve_at("/org/freedesktop/login1", Login { calls: state }).build().await.unwrap()
    });
    let client = zbus::connection::Builder::unix_stream(client_socket.into_std().unwrap()).p2p().build().await.unwrap();
    (client, server.await.unwrap(), calls)
}

#[tokio::test]
async fn diagnostics_read_jobs_and_only_shutdown_inhibitors_without_power_or_inhibit_calls() {
    let (connection, _server, calls) = fixture(false).await;
    let pending = jobs(&connection).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!((pending[0].id, pending[0].unit.as_str(), pending[0].kind.as_str(), pending[0].state.as_str()),
        (17, "slow.service", "stop", "running"));
    let status = logind_status(&connection).await.unwrap();
    assert!(status.preparing);
    assert_eq!(status.inhibitors.iter().map(|lock| lock.pid).collect::<Vec<_>>(), [11, 12]);
    assert_eq!(status.inhibitors[0].why, "Saving files");
    assert_eq!(*calls.lock().unwrap(), ["ListJobs", "ListInhibitors"]);
}

#[tokio::test]
async fn an_unresponsive_manager_does_not_hang_diagnostics() {
    let (connection, _server, _) = fixture(true).await;
    let before = Instant::now();
    let error = jobs(&connection).await.unwrap_err();
    assert!(error.to_string().contains("shutdown diagnostics timed out"), "{error}");
    assert!(before.elapsed() < Duration::from_secs(3));
    assert!(logind_status(&connection).await.unwrap().preparing);
}

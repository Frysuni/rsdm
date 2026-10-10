use std::{io::Read, os::{fd::AsRawFd, unix::net::UnixStream}};

use super::*;

fn pending() -> (Handle, oneshot::Sender<()>) {
    let (_events, notices) = mpsc::unbounded_channel();
    let (finished, completion) = oneshot::channel();
    (Handle { sender: None, finished: Some(completion), notices, interrupt: Arc::default(),
        connected: HashSet::new(), environment: Vec::new() }, finished)
}

#[tokio::test]
async fn cancelling_pending_startup_interrupts_sockets_without_waiting_for_worker_completion() {
    let (handle, _finished) = pending();
    let (reader, mut writer) = UnixStream::pair().unwrap();
    let _socket = handle.interrupt.register(reader.as_raw_fd()).unwrap();
    let (_ready, started) = oneshot::channel();
    let mut startup = Box::pin(async move {
        let mut handle = handle;
        handle.await_startup(started, &Deadline::default()).await.unwrap();
        handle
    });
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(10)) => {},
        _ = &mut startup => panic!("startup completed without the ready handshake"),
    }
    let before = std::time::Instant::now();
    drop(startup);
    assert!(before.elapsed() < Duration::from_millis(100));
    assert_eq!(writer.read(&mut [0_u8]).unwrap(), 0);
}

#[tokio::test]
async fn completion_wait_keeps_the_callers_runtime_responsive() {
    let (mut handle, finished) = pending();
    let deadline = Deadline::default();
    let mut shutdown = Box::pin(handle.shutdown(&deadline));
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(10)) => {},
        result = &mut shutdown => panic!("pending worker was treated as finished: {result:?}"),
    }
    finished.send(()).unwrap();
    shutdown.await.unwrap();
    assert!(handle.finished.is_none());
}

#[tokio::test]
async fn expired_budgets_interrupt_sockets_but_retain_unconfirmed_completion() {
    let (mut handle, finished) = pending();
    let (reader, mut writer) = UnixStream::pair().unwrap();
    let _socket = handle.interrupt.register(reader.as_raw_fd()).unwrap();
    let deadline = Deadline::default();
    deadline.set(1);
    assert!(handle.shutdown(&deadline).await.is_err());
    assert_eq!(writer.read(&mut [0_u8]).unwrap(), 0);
    assert!(handle.finished.is_some());
    finished.send(()).unwrap();
    handle.shutdown(&deadline).await.unwrap();
    assert!(handle.finished.is_none());
}

struct PausedManager(Arc<tokio::sync::Notify>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl PausedManager {
    #[zbus(name = "GetUnitByPID")]
    async fn get_unit_by_pid(&self, _pid: u32) -> zbus::zvariant::OwnedObjectPath {
        self.0.notify_one();
        std::future::pending().await
    }
}

async fn paused_manager() -> (UserManager, zbus::Connection, Arc<tokio::sync::Notify>) {
    let entered = Arc::new(tokio::sync::Notify::new());
    let manager = PausedManager(entered.clone());
    let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket).p2p().server(zbus::Guid::generate()).unwrap()
            .serve_at("/org/freedesktop/systemd1", manager).unwrap().build().await.unwrap()
    });
    let connection = zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap();
    (UserManager::with_connection(connection), server.await.unwrap(), entered)
}

struct Directory(std::path::PathBuf);

impl Directory {
    fn create() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("rsdm-xsmp-{}", zbus::Guid::generate()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

#[tokio::test]
async fn shutdown_closes_an_accepted_ice_connection_while_ownership_admission_is_pending() {
    use tokio::io::AsyncReadExt;

    let (manager, _server, entered) = paused_manager().await;
    let directory = Directory::create();
    let runtime = Runtime { path: directory.0.clone(), generation: "0123456789abcdef0123456789abcdef".into() };
    let provider = Provider { kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new() };
    let mut handle = Handle::start(&manager, &runtime, &provider).await.unwrap();
    let networks = &handle.environment.iter().find(|(name, _)| name == "SESSION_MANAGER").unwrap().1;
    let endpoint = networks.split(',').find(|network| network.starts_with("unix/")).unwrap()
        .split_once(':').unwrap().1.to_string();
    let mut client = tokio::net::UnixStream::connect(&endpoint).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();

    handle.shutdown(&Deadline::default()).await.unwrap();
    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), client.read_to_end(&mut received)).await.unwrap().unwrap();
    assert!(!runtime.path.join("ICEauthority").exists());
    let sockets = std::fs::read_to_string("/proc/net/unix").unwrap();
    assert!(!sockets.lines().any(|socket| socket.ends_with(&endpoint)), "accepted ICE socket leaked after worker shutdown");
}

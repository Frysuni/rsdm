use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};

use rsdm_core::domain::{SessionPhase, ShutdownPolicy};
use zbus::fdo::ConnectionCredentials;

use super::*;

const GENERATION: &str = "0123456789abcdef0123456789abcdef";
const UNIT: &str = "app-rsdm-handshake@0123456789abcdef0123456789abcdef-1.service";
const PATH: &str = "/org/freedesktop/systemd1/unit/app";

struct Manager;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    #[zbus(name = "GetUnitByPID")]
    fn get_unit_by_pid(&self, pid: u32) -> OwnedObjectPath {
        assert_eq!(pid, std::process::id());
        OwnedObjectPath::try_from(PATH).unwrap()
    }
}

struct Unit { name: String, invocation: Vec<u8>, replace: bool, reads: AtomicUsize }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property)]
    fn id(&self) -> &str { &self.name }

    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst);
        if self.replace && read != 0 { vec![2; 16] } else { self.invocation.clone() }
    }
}

struct Service(String);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property)]
    fn control_group(&self) -> &str { &self.0 }
}

struct Scope(String);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Scope")]
impl Scope {
    #[zbus(property)]
    fn control_group(&self) -> &str { &self.0 }
}

struct Fixture {
    directory: PathBuf,
    runtime: Runtime,
    record: SessionRecord,
    connection: Connection,
    _server: Connection,
}

impl Fixture {
    async fn new(invocation: Vec<u8>, replace: bool, group: String) -> Self {
        Self::in_unit(UNIT, invocation, replace, group).await
    }

    async fn in_unit(name: &str, invocation: Vec<u8>, replace: bool, group: String) -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-handshake-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(directory.join("apps")).unwrap();
        let runtime = Runtime { path: directory.clone(), generation: GENERATION.into() };
        runtime.save_app(&super::super::runtime::AppRecord {
            unit: UNIT.into(), invocation_id: vec![1; 16], policy: ShutdownPolicy::default(),
            deadline_usec: None, quit_started: false, dispatch: None, window_close_sent: false, original_process: None, migrated_scope: None, lifetime_process: None, preparation: None,
        }).unwrap();
        let (server, client) = tokio::net::UnixStream::pair().unwrap();
        let name = name.to_string();
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server.into_std().unwrap()).p2p().server(zbus::Guid::generate())
                .serve_at("/org/freedesktop/systemd1", Manager)
                .serve_at(PATH, Unit { name, invocation, replace, reads: AtomicUsize::new(0) })
                .serve_at(PATH, Scope(group.clone()))
                .serve_at(PATH, Service(group)).build().await.unwrap()
        });
        let connection = zbus::connection::Builder::unix_stream(client.into_std().unwrap()).p2p().build().await.unwrap();
        let identity = SessionIdentity { generation: GENERATION.into(), uid: unsafe { libc::geteuid() }, login_session_id: "test".into(), desktop_entry_id: None };
        let record = SessionRecord {
            identity, anchor_unit: format!("rsdm-session-{GENERATION}.service"), compositor_unit: None,
            compositor_invocation: Vec::new(), coordinator_unit: None, provider: "managed".into(), owns_targets: true,
            phase: SessionPhase::Running, disabled: false, config_path: None, exported_environment: Vec::new(), shutdown_deadline_usec: None, shutdown_transaction: None,
        };
        Self { directory, runtime, record, connection, _server: server.await.unwrap() }
    }

    async fn verify(&self) -> Result<(), SessionError> {
        let credentials = ConnectionCredentials::default().set_unix_user_id(self.record.identity.uid).set_process_id(std::process::id());
        let caller = Caller::from_credentials(&credentials, self.record.identity.uid).unwrap();
        registered_process(&self.connection, &caller, &self.runtime, &self.record).await
    }
}

#[tokio::test]
async fn shared_user_scopes_and_services_require_live_pinned_cgroup_membership() {
    for name in ["app-compositor-terminal.scope", "desktop-launcher.service"] {
        for (invocation, replace, group, accepted) in [
            (vec![1; 16], false, current_group(), true),
            (vec![1; 16], true, current_group(), false),
            (vec![0; 16], false, current_group(), false),
            (Vec::new(), false, current_group(), false),
            (vec![1; 16], false, "/another-session".into(), false),
            (vec![1; 16], false, "/".into(), false),
        ] {
            let fixture = Fixture::in_unit(name, invocation, replace, group).await;
            let credentials = ConnectionCredentials::default().set_unix_user_id(fixture.record.identity.uid).set_process_id(std::process::id());
            let caller = Caller::from_credentials(&credentials, fixture.record.identity.uid).unwrap();
            assert_eq!(user_unit(&fixture.connection, &caller).await.is_ok(), accepted, "{name}");
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

fn current_group() -> String {
    fs::read_to_string("/proc/self/cgroup").unwrap().lines()
        .find_map(|line| line.strip_prefix("0::")).unwrap().to_string()
}

#[tokio::test]
async fn another_or_replaced_invocation_is_rejected() {
    for (invocation, replace) in [(vec![2; 16], false), (vec![1; 16], true), (Vec::new(), false)] {
        let fixture = Fixture::new(invocation, replace, current_group()).await;
        assert!(fixture.verify().await.is_err());
    }
}

#[tokio::test]
async fn a_unit_name_without_registration_or_membership_is_insufficient() {
    for group in ["/another-session", "/", ""] {
        let fixture = Fixture::new(vec![1; 16], false, group.into()).await;
        assert!(fixture.verify().await.is_err());
    }
    let fixture = Fixture::new(vec![1; 16], false, current_group()).await;
    fs::remove_file(fixture.runtime.path.join("apps").join(format!("{UNIT}.toml"))).unwrap();
    assert!(fixture.verify().await.is_err());
}

#[tokio::test]
async fn an_early_compositor_caller_waits_for_the_recorded_boot_invocation() {
    let fixture = Fixture::new(vec![1; 16], false, current_group()).await;
    let mut record = fixture.record.clone();
    record.phase = SessionPhase::Starting;
    record.compositor_unit = Some(UNIT.into());
    fixture.runtime.save_session(&record).unwrap();
    let runtime = fixture.runtime.clone();
    let expected = record.clone();
    let waiting = tokio::spawn(async move { compositor_invocation(&runtime, &expected).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiting.is_finished());
    record.compositor_invocation = vec![1; 16];
    fixture.runtime.save_session(&record).unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(1), waiting).await.unwrap().unwrap().unwrap(), vec![1; 16]);
}

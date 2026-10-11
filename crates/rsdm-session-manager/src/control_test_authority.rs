//! Private logind and recovery records used by control transport fixtures.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::{Arc, atomic::{AtomicBool, Ordering}}};

use rsdm_core::domain::SessionPhase;
use zbus::{Connection, zvariant::OwnedObjectPath};

use super::{identity::SessionIdentity, runtime::{Runtime, SessionRecord}};

const PATH: &str = "/org/freedesktop/login1/session/test";

struct Manager { pending: Arc<AtomicBool>, entered: Arc<tokio::sync::Notify> }

#[zbus_macros::interface(name = "org.freedesktop.login1.Manager")]
impl Manager {
    #[zbus(name = "GetSessionByPID")]
    async fn get_session_by_pid(&self, pid: u32) -> zbus::fdo::Result<OwnedObjectPath> {
        if pid != std::process::id() { return Err(zbus::fdo::Error::AccessDenied("unexpected caller process".into())); }
        if self.pending.load(Ordering::SeqCst) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(OwnedObjectPath::try_from(PATH).unwrap())
    }
}

struct Session(SessionIdentity);

#[zbus_macros::interface(name = "org.freedesktop.login1.Session")]
impl Session {
    #[zbus(property)]
    fn user(&self) -> (u32, OwnedObjectPath) {
        (self.0.uid, OwnedObjectPath::try_from("/org/freedesktop/login1/user/test").unwrap())
    }

    #[zbus(property)]
    fn id(&self) -> &str { &self.0.login_session_id }
}

pub(super) struct Context {
    pub(super) runtime: Runtime,
    pub(super) logind: Connection,
    pub(super) pending_login: Arc<AtomicBool>,
    pub(super) login_entered: Arc<tokio::sync::Notify>,
    _server: Connection,
    directory: PathBuf,
}

impl Context {
    pub(super) async fn create(identity: SessionIdentity) -> Arc<Self> {
        let directory = std::env::temp_dir().join(format!("rsdm-control-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::create_at(&directory, &identity.generation).unwrap();
        runtime.save_session(&SessionRecord {
            anchor_unit: format!("rsdm-session-{}.service", identity.generation), identity: identity.clone(),
            compositor_unit: None, compositor_invocation: Vec::new(), provider: "managed".into(),
            owns_targets: true, phase: SessionPhase::Running, disabled: false, config_path: None,
            exported_environment: Vec::new(), shutdown_deadline_usec: None,
        }).unwrap();

        let (server, client) = tokio::net::UnixStream::pair().unwrap();
        let pending_login = Arc::new(AtomicBool::new(false));
        let login_entered = Arc::new(tokio::sync::Notify::new());
        let manager = Manager { pending: pending_login.clone(), entered: login_entered.clone() };
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server).p2p().server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/login1", manager).unwrap()
                .serve_at(PATH, Session(identity)).unwrap().build().await.unwrap()
        });
        let logind = zbus::connection::Builder::unix_stream(client).p2p().build().await.unwrap();
        Arc::new(Self { runtime, logind, pending_login, login_entered, _server: server.await.unwrap(), directory })
    }

    pub(super) async fn set_login(&self, identity: SessionIdentity) {
        self._server.object_server().remove::<Session, _>(PATH).await.unwrap();
        self._server.object_server().at(PATH, Session(identity)).await.unwrap();
    }
}

impl Drop for Context {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

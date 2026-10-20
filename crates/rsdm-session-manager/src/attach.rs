//! Start one independent user service and join its authenticated generation.

use std::{path::Path, time::Duration};

use rsdm_core::domain::SessionManagerConfig;
use zbus::{Fd, Variant};

use super::{SessionError, action_gate::ActionGate, attach_bootstrap::Snapshot, bus::UserManager, control, env, identity::SessionIdentity, runtime, units};

pub(super) const UNIT: &str = "rsdm-session-manager.service";

struct BootstrapTask(tokio::task::JoinHandle<zbus::Result<zbus::Connection>>);

impl Drop for BootstrapTask {
    fn drop(&mut self) { self.0.abort(); }
}

pub(super) async fn attach(cfg: &SessionManagerConfig, path: &Path, actions: ActionGate) -> Result<String, SessionError> {
    if !cfg.enabled { return Err(SessionError::Disabled); }
    actions.check()?;
    let connected = tokio::time::timeout(Duration::from_secs(5), zbus::connection::Builder::session().method_timeout(Duration::from_secs(5)).build());
    let connection = actions.guard(connected).await?
        .map_err(|_| SessionError::State("connecting for attach timed out".into()))??;
    if control_exists(&connection).await? { return join(&connection, actions).await; }
    let manager = UserManager::connect_guarded(super::deadline::Deadline::default(), actions.clone()).await?;
    let snapshot = Snapshot {
        identity: actions.guard(SessionIdentity::discover_attach()).await??,
        environment: env::command_environment(), directory: runtime::blocking(|| Ok(std::env::current_dir()?)).await?,
    };
    let (server, descriptor) = super::attach_bootstrap::channel(snapshot.clone())?;
    let server = BootstrapTask(server);
    let executable = runtime::blocking(|| Ok(std::env::current_exe()?)).await?;
    let path = path.to_path_buf();
    let path = runtime::blocking(move || Ok(std::path::absolute(path)?)).await?;
    let argv = vec![
        super::bus_path::encode(&executable)?, "--config".into(), super::bus_path::encode(&path)?,
        "session".into(), "serve".into(),
    ];
    let environment = snapshot.identity.command_environment(snapshot.environment.clone());
    let mut properties = units::command_properties(&argv, &environment, &snapshot.directory).await?;
    properties.extend([("StandardInputFileDescriptor", Variant::new(Fd::from(descriptor))), ("Slice", Variant::from("app.slice"))]);
    match manager.start_service(UNIT, &properties).await {
        Ok(()) => {},
        Err(SessionError::StartRejected { error, .. }) if matches!(error.as_ref(), SessionError::Bus(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.systemd1.UnitExists") => {},
        Err(error) => return Err(error),
    }
    let generation = join(&manager.connection(), actions).await?;
    drop(server);
    Ok(generation)
}

pub(super) async fn control_exists(connection: &zbus::Connection) -> Result<bool, SessionError> {
    let reply = connection.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "NameHasOwner", &(control::BUS_NAME,)).await?;
    Ok(reply.body().deserialize()?)
}

async fn join(connection: &zbus::Connection, actions: ActionGate) -> Result<String, SessionError> {
    actions.guard(tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let result = control::proxy(connection).await?.call::<_, _, super::control::SessionStatus>("Attach", &()).await;
            match result {
                Ok(status) => return Ok(status.generation),
                Err(error) if super::bus::retryable_error(&error) || error.to_string().contains("session is not published") => tokio::time::sleep(Duration::from_millis(50)).await,
                Err(error) => return Err(SessionError::from(error)),
            }
        }
    })).await?.map_err(|_| SessionError::State("attached coordinator did not become ready".into()))?
}

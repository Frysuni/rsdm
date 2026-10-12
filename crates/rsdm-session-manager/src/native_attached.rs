//! Delegate attached logout only to live peers in the attached logind session.

use std::time::Duration;

use zbus::{Connection, proxy::MethodFlags};

use super::{SessionError, app_stop::ShutdownControl, bus::UserManager, identity::SessionIdentity,
    native_delegate::{Peer, pin_peer}, provider::ProviderKind, runtime::Runtime};

pub(super) struct Target { kind: ProviderKind, connection: Connection, peer: Peer, desktop: Option<Peer> }

pub(super) struct Scoped { target: Target, logind: Connection }

impl Target {
    async fn verify(&self, logind: &Connection, identity: &SessionIdentity) -> zbus::Result<()> {
        if let Some(desktop) = &self.desktop { desktop.verify_login(&self.connection, logind, identity).await?; }
        self.peer.verify_login(&self.connection, logind, identity).await
    }
}

impl Scoped {
    pub(super) async fn logout(&self, manager: &UserManager, runtime: &Runtime, identity: &SessionIdentity,
        control: &ShutdownControl) -> Result<(), SessionError> {
        let (path, interface, method) = match self.target.kind {
            ProviderKind::Gnome => ("/org/gnome/SessionManager", "org.gnome.SessionManager", "Logout"),
            ProviderKind::Plasma => ("/LogoutPrompt", "org.kde.LogoutPrompt", "promptLogout"),
            _ => return Err(SessionError::State("unsupported attached native provider".into())),
        };
        manager.request(async {
            let records = runtime.clone();
            let record = super::runtime::blocking(move || records.session()).await
                .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
            if record.identity != *identity || record.disabled || record.phase != rsdm_core::domain::SessionPhase::StoppingSession
                || record.pending_transaction_id() != control.transaction_id.as_deref()
            { return Err(zbus::fdo::Error::AccessDenied("native logout transaction is stale or disabled".into()).into()); }
            let proxy = zbus::Proxy::new(&self.target.connection, self.target.peer.owner.as_str(), path, interface).await?;
            self.target.verify(&self.logind, identity).await?;
            manager.deadline.remaining(Duration::MAX)?;
            manager.actions.check().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
            if control.cancelled() || control.recovery.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(zbus::fdo::Error::Failed("native logout preparation was cancelled or entered recovery".into()).into());
            }
            if method == "Logout" {
                proxy.call_with_flags::<_, _, ()>(method, MethodFlags::NoAutoStart.into(), &(0_u32,)).await?;
            } else {
                proxy.call_with_flags::<_, _, ()>(method, MethodFlags::NoAutoStart.into(), &()).await?;
            }
            Ok(())
        }).await.map_err(Into::into)
    }
}

pub(super) async fn discover(manager: &UserManager, identity: &SessionIdentity) -> Result<Option<Scoped>, SessionError> {
    let targets = manager.read(|| async { read_targets(manager.connection()).await }).await?;
    if targets.is_empty() { return Ok(None); }
    let logind = manager.request(async {
        zbus::connection::Builder::system()?.method_timeout(Duration::from_secs(2)).build().await
    }).await?;
    scope_targets(manager, identity, logind, targets).await
}

pub(super) async fn prepare(manager: &UserManager, identity: &SessionIdentity,
    control: &ShutdownControl) -> Result<Option<Scoped>, SessionError> {
    tokio::select! {
        result = discover(manager, identity) => result,
        _ = async {
            while !control.cancelled() && !control.recovery.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        } => Err(SessionError::State("native logout lookup was cancelled or entered recovery".into())),
    }
}

pub(super) async fn scope_targets(manager: &UserManager, identity: &SessionIdentity, logind: Connection,
    targets: Vec<Target>) -> Result<Option<Scoped>, SessionError> {
    let mut rejected = None;
    for target in targets {
        match manager.request(target.verify(&logind, identity)).await {
            Ok(()) => return Ok(Some(Scoped { target, logind })),
            Err(error) => rejected = Some(error),
        }
    }
    match rejected { Some(error) => Err(error.into()), None => Ok(None) }
}

pub(super) async fn read_targets(connection: Connection) -> zbus::Result<Vec<Target>> {
    let mut targets = Vec::new();
    let mut rejected = None;
    for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
        match read_target(&connection, kind).await {
            Ok(target) => targets.push(target),
            Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner" => {},
            Err(error) => rejected = Some(error),
        }
    }
    if targets.is_empty() && let Some(error) = rejected { return Err(error); }
    Ok(targets)
}

async fn read_target(connection: &Connection, kind: ProviderKind) -> zbus::Result<Target> {
    let (name, desktop) = match kind {
        ProviderKind::Gnome => ("org.gnome.SessionManager", None),
        ProviderKind::Plasma => ("org.kde.LogoutPrompt", Some(pin_peer(connection, "org.kde.ksmserver").await?)),
        _ => return Err(zbus::fdo::Error::NotSupported("unsupported attached native provider".into()).into()),
    };
    let peer = pin_peer(connection, name).await?;
    Ok(Target { kind, connection: connection.clone(), peer, desktop })
}

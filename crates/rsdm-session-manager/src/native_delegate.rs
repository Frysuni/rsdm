//! Native shutdown targets one authenticated owner on the original bus connection.

use zbus::{Connection, names::OwnedUniqueName, proxy::MethodFlags};

use super::{SessionError, bus::UserManager, control_caller::Caller, provider::{Provider, ProviderKind}};

pub(super) struct Peer { service: &'static str, pub(super) owner: OwnedUniqueName, process: Caller, broker_pinned: bool }

impl Peer {
    pub(super) async fn verify_login(&self, connection: &Connection, logind: &Connection,
        identity: &super::identity::SessionIdentity) -> zbus::Result<()> {
        if !self.broker_pinned {
            return Err(zbus::fdo::Error::AccessDenied("native login ownership requires a broker-provided ProcessFD".into()).into());
        }
        let belongs = self.process.belongs_to_login(logind, identity).await
            .map_err(|error| match error {
                SessionError::Bus(error) => error,
                error => zbus::fdo::Error::AccessDenied(error.to_string()).into(),
            })?;
        if !belongs { return Err(zbus::fdo::Error::AccessDenied("native desktop process has no verified login session".into()).into()); }
        self.verify(connection).await
    }

    async fn verify(&self, connection: &Connection) -> zbus::Result<()> {
        if current_owner(connection, self.service).await? != self.owner {
            return Err(zbus::fdo::Error::AccessDenied("native desktop bus owner changed before shutdown dispatch".into()).into());
        }
        self.process.check_alive().map_err(|error| zbus::fdo::Error::AccessDenied(error.to_string()))?;
        Ok(())
    }
}

impl Provider {
    pub(super) async fn logout_available(&self, manager: &UserManager) -> Result<bool, SessionError> {
        manager.read(|| async {
            let connection = manager.connection();
            match probe(&connection, self.kind).await {
                Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner" => Ok(false),
                result => result,
            }
        }).await
    }

    pub async fn delegate(&self, manager: &UserManager, action: &str) -> Result<(), SessionError> {
        let (name, path, interface) = match self.kind {
            ProviderKind::Gnome => ("org.gnome.SessionManager", "/org/gnome/SessionManager", "org.gnome.SessionManager"),
            ProviderKind::Plasma => ("org.kde.LogoutPrompt", "/LogoutPrompt", "org.kde.LogoutPrompt"),
            _ => return Err(SessionError::State("this provider does not delegate desktop shutdown".into())),
        };
        let method = match (self.kind, action) {
            (ProviderKind::Gnome, "logout") => "Logout",
            (ProviderKind::Gnome, "reboot") => "Reboot",
            (ProviderKind::Gnome, "poweroff") => "Shutdown",
            (ProviderKind::Plasma, "logout") => "promptLogout",
            (ProviderKind::Plasma, "reboot") => "promptReboot",
            (ProviderKind::Plasma, "poweroff") => "promptShutDown",
            _ => return Err(SessionError::State("unsupported native session action".into())),
        };
        manager.deadline.bound(async {
            let connection = manager.connection();
            let (peer, desktop) = recipient(manager, &connection, self.kind, name).await?;
            let proxy = manager.request(zbus::Proxy::new(&connection, peer.owner.as_str(), path, interface)).await?;
            manager.request(async {
                if let Some(desktop) = &desktop { desktop.verify(&connection).await?; }
                peer.verify(&connection).await?;
                manager.actions.check().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
                if method == "Logout" {
                    proxy.call_with_flags::<_, _, ()>(method, MethodFlags::NoAutoStart.into(), &(0_u32,)).await?;
                } else {
                    proxy.call_with_flags::<_, _, ()>(method, MethodFlags::NoAutoStart.into(), &()).await?;
                }
                Ok(())
            }).await?;
            Ok(())
        }).await
    }
}

async fn recipient(manager: &UserManager, connection: &Connection, kind: ProviderKind, name: &'static str) -> Result<(Peer, Option<Peer>), SessionError> {
    let desktop = if kind == ProviderKind::Plasma { Some(pin(manager, connection, "org.kde.ksmserver").await?) } else { None };
    let peer = match pin(manager, connection, name).await {
        Ok(peer) => peer,
        Err(SessionError::Bus(zbus::Error::MethodError(error, _, _)))
            if kind == ProviderKind::Plasma && error.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner" => {
            let desktop = desktop.as_ref().expect("verified Plasma desktop");
            manager.request(async {
                desktop.verify(connection).await?;
                manager.actions.check().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
                let reply = connection.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"), "StartServiceByName", &(name, 0_u32)).await?;
                let _: u32 = reply.body().deserialize()?;
                Ok(())
            }).await?;
            pin(manager, connection, name).await?
        }
        Err(error) => return Err(error),
    };
    Ok((peer, desktop))
}

async fn pin(manager: &UserManager, connection: &Connection, name: &'static str) -> Result<Peer, SessionError> {
    manager.request(pin_peer(connection, name)).await.map_err(Into::into)
}

pub(super) async fn pin_peer(connection: &Connection, name: &'static str) -> zbus::Result<Peer> {
    let owner = current_owner(connection, name).await?;
    let reply = connection.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"), "GetConnectionCredentials", &(owner.as_str(),)).await?;
    let credentials = reply.body().deserialize::<zbus::fdo::ConnectionCredentials>()?;
    // SAFETY: geteuid has no preconditions; native shutdown stays within this UID.
    let process = Caller::from_credentials(&credentials, unsafe { libc::geteuid() })
        .map_err(|error| zbus::fdo::Error::AccessDenied(error.to_string()))?;
    Ok(Peer { service: name, owner, process, broker_pinned: credentials.process_fd().is_some() })
}

async fn probe(connection: &Connection, kind: ProviderKind) -> zbus::Result<bool> {
    let name = match kind {
        ProviderKind::Gnome => "org.gnome.SessionManager",
        ProviderKind::Plasma => "org.kde.LogoutPrompt",
        _ => return Ok(false),
    };
    let desktop = if kind == ProviderKind::Plasma { Some(pin_peer(connection, "org.kde.ksmserver").await?) } else { None };
    let (available, peer) = match pin_peer(connection, name).await {
        Ok(peer) => (true, Some(peer)),
        Err(zbus::Error::MethodError(error, _, _))
            if kind == ProviderKind::Plasma && error.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner" => {
            let reply = connection.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"), "ListActivatableNames", &()).await?;
            let names: Vec<String> = reply.body().deserialize()?;
            (names.iter().any(|name| name == "org.kde.LogoutPrompt"), None)
        }
        Err(error) => return Err(error),
    };
    if let Some(desktop) = desktop { desktop.verify(connection).await?; }
    if let Some(peer) = peer { peer.verify(connection).await?; }
    Ok(available)
}

async fn current_owner(connection: &Connection, name: &str) -> zbus::Result<OwnedUniqueName> {
    let reply = connection.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"), "GetNameOwner", &(name,)).await?;
    reply.body().deserialize()
}

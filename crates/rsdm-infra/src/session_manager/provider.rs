//! Desktop lifecycle ownership; application-specific behavior belongs to callers.

use std::path::Path;

use rsdm_core::domain::SessionMode;

use super::{SessionError, bus::UserManager};

#[derive(Debug, Default, Clone)]
pub struct StartOptions {
    pub mode: SessionMode,
    pub native_unit: Option<String>,
    pub logout_command: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProviderKind {
    Managed,
    Niri,
    Gnome,
    Plasma,
    External,
}

#[derive(Debug, Clone)]
pub(super) struct Provider {
    pub kind: ProviderKind,
    pub native_unit: Option<String>,
    pub logout_command: Vec<String>,
}

impl Provider {
    pub fn select(argv: &[String], desktop: &str, options: &StartOptions) -> Result<Self, SessionError> {
        if let Some(unit) = &options.native_unit {
            validate_native_unit(unit)?;
        }
        let kind = match options.mode {
            SessionMode::Managed => ProviderKind::Managed,
            SessionMode::External => ProviderKind::External,
            SessionMode::Auto if options.native_unit.is_some() || !options.logout_command.is_empty() => ProviderKind::External,
            SessionMode::Auto => detect_provider(argv, desktop),
        };
        if kind == ProviderKind::Managed && options.native_unit.is_some() {
            return Err(SessionError::State("managed mode cannot observe a native unit".into()));
        }
        let native_unit = options.native_unit.clone().or_else(|| {
            (kind == ProviderKind::Niri).then(|| "niri.service".to_string())
        });
        Ok(Self { kind, native_unit, logout_command: options.logout_command.clone() })
    }

    pub fn name(&self) -> &'static str {
        match self.kind {
            ProviderKind::Managed => "managed",
            ProviderKind::Niri => "niri",
            ProviderKind::Gnome => "gnome",
            ProviderKind::Plasma => "plasma",
            ProviderKind::External => "external",
        }
    }

    pub fn native_desktop(&self) -> bool {
        matches!(self.kind, ProviderKind::Gnome | ProviderKind::Plasma)
    }

    pub fn ready(&self, manager: &UserManager) -> Result<bool, SessionError> {
        if let Some(unit) = &self.native_unit {
            return manager.active(unit);
        }
        match self.kind {
            ProviderKind::Gnome => native_gnome_running(&manager.connection),
            ProviderKind::Plasma => native_name_owned(&manager.connection, "org.kde.ksmserver"),
            _ => manager.active(super::units::SESSION_TARGET),
        }
    }

    pub fn delegate(&self, manager: &UserManager, action: &str) -> Result<(), SessionError> {
        async_io::block_on(async {
            let connection = &manager.connection;
            match self.kind {
                ProviderKind::Gnome => {
                    let proxy = zbus::Proxy::new(&connection, "org.gnome.SessionManager", "/org/gnome/SessionManager", "org.gnome.SessionManager").await?;
                    match action {
                        "logout" => proxy.call::<_, _, ()>("Logout", &(0_u32,)).await?,
                        "reboot" => proxy.call::<_, _, ()>("Reboot", &()).await?,
                        "poweroff" => proxy.call::<_, _, ()>("Shutdown", &()).await?,
                        _ => return Err(SessionError::State("unsupported native session action".into())),
                    }
                }
                ProviderKind::Plasma => {
                    let method = match action {
                        "logout" => "promptLogout",
                        "reboot" => "promptReboot",
                        "poweroff" => "promptShutDown",
                        _ => return Err(SessionError::State("unsupported native session action".into())),
                    };
                    let proxy = zbus::Proxy::new(&connection, "org.kde.LogoutPrompt", "/LogoutPrompt", "org.kde.LogoutPrompt").await?;
                    proxy.call::<_, _, ()>(method, &()).await?;
                }
                _ => return Err(SessionError::State("this provider does not delegate desktop shutdown".into())),
            }
            Ok(())
        })
    }
}

fn detect_provider(argv: &[String], desktop: &str) -> ProviderKind {
    let program = argv.first().and_then(|program| Path::new(program).file_name()).and_then(|name| name.to_str());
    if program == Some("niri-session") {
        return ProviderKind::Niri;
    }
    for name in desktop.split(':') {
        if name.eq_ignore_ascii_case("gnome") { return ProviderKind::Gnome; }
        if name.eq_ignore_ascii_case("kde") || name.eq_ignore_ascii_case("plasma") { return ProviderKind::Plasma; }
    }
    ProviderKind::Managed
}

fn native_gnome_running(connection: &zbus::Connection) -> Result<bool, SessionError> {
    if !native_name_owned(connection, "org.gnome.SessionManager")? { return Ok(false); }
    async_io::block_on(async {
        let proxy = zbus::Proxy::new(connection, "org.gnome.SessionManager", "/org/gnome/SessionManager", "org.gnome.SessionManager").await?;
        Ok(proxy.call("IsSessionRunning", &()).await?)
    })
}

fn native_name_owned(connection: &zbus::Connection, name: &str) -> Result<bool, SessionError> {
    async_io::block_on(async {
        let proxy = zbus::Proxy::new(connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
        Ok(proxy.call("NameHasOwner", &(name,)).await?)
    })
}

fn validate_native_unit(unit: &str) -> Result<(), SessionError> {
    if !unit.ends_with(".service") || unit.len() > 255 || unit.contains('/') || unit.contains('\0') {
        return Err(SessionError::State("native unit must be a service unit name".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_official_niri_wrapper_selects_the_native_unit() {
        let options = StartOptions::default();
        assert_eq!(Provider::select(&["/path/niri-session".into()], "niri", &options).unwrap().native_unit.as_deref(), Some("niri.service"));
        assert_eq!(Provider::select(&["niri".into()], "niri", &options).unwrap().kind, ProviderKind::Managed);
    }

    #[test]
    fn explicit_mode_overrides_desktop_hints_without_replacing_argv() {
        let options = StartOptions { mode: SessionMode::Managed, ..StartOptions::default() };
        assert_eq!(Provider::select(&["session-command".into()], "GNOME", &options).unwrap().kind, ProviderKind::Managed);
        let options = StartOptions { mode: SessionMode::External, native_unit: Some("custom-session.service".into()), ..StartOptions::default() };
        assert_eq!(Provider::select(&["session-command".into()], "GNOME", &options).unwrap().native_unit.as_deref(), Some("custom-session.service"));
    }
}

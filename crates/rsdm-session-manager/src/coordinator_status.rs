//! Read-only status snapshots and bounded capability probes run outside the actor.

use std::{collections::BTreeMap, time::Duration};

use rsdm_core::domain::ShutdownMethod;

use crate::{SessionError, bus::UserManager, control::{Reply, SessionStatus},
    coordinator::{Coordinator, Work, MAX_COORDINATOR_WORKERS},
    runtime::{AppRecord, Runtime, SessionRecord, blocking}, status::{self, Capability, SessionStatusDetails}};

impl Coordinator {
    pub(super) async fn status(&self) -> Result<SessionStatus, SessionError> {
        let runtime = self.runtime.clone();
        let apps = blocking(move || runtime.apps()).await?;
        let mut status = self.status_summary();
        status.apps = apps.iter().map(|app| {
            let method = if !app.policy.quit_command.is_empty() { "command".into() }
                else if app.policy.method != ShutdownMethod::Auto { app.policy.method.to_string() }
                else if self.provider.native_desktop() { "native".into() }
                else if self.xsmp.connected.contains(&app.unit) { "xsmp".into() }
                else { "term".into() };
            (app.unit.clone(), method, app.policy.timeout_secs, app_state(app).into())
        }).collect();
        Ok(status)
    }

    fn status_summary(&self) -> SessionStatus {
        SessionStatus { generation: self.runtime.generation.clone(),
            login_session_id: self.record.identity.login_session_id.clone(),
            desktop_entry_id: self.record.identity.desktop_entry_id.clone().unwrap_or_default(),
            provider: self.provider.name().into(), phase: self.lifecycle.phase.to_string(),
            xsmp_available: self.xsmp.available(), apps: Vec::new() }
    }

    pub(super) fn request_status_details(&mut self, reply: Reply<SessionStatusDetails>) {
        if self.workers >= MAX_COORDINATOR_WORKERS {
            let _ = reply.try_send(Err("session coordinator is busy; retry the status query".into()));
            return;
        }
        let manager = self.manager.clone();
        let runtime = self.runtime.clone();
        let record = self.record.clone();
        let status = self.status_summary();
        let events = self.events.clone();
        self.workers += 1;
        tokio::spawn(async move {
            let result = snapshot(&manager, runtime, record, status).await;
            let _ = events.send(Work::StatusDetails(reply, result));
        });
    }
}

async fn snapshot(manager: &UserManager, runtime: Runtime, record: SessionRecord, mut status: SessionStatus) -> Result<SessionStatusDetails, SessionError> {
    if record.disabled || manager.actions.check().is_err() { return Ok(SessionStatusDetails::disabled()); }
    let (apps, archived) = blocking(move || Ok((runtime.apps()?, runtime.archived_apps()?))).await?;
    status.apps = apps.iter().map(|app| {
        let method = if let Some(dispatch) = app.dispatch.as_ref().filter(|dispatch| dispatch.invocation_id == app.invocation_id) { status::method(dispatch.method).into() }
            else if !app.policy.quit_command.is_empty() { "command".into() } else { app.policy.method.to_string() };
        (app.unit.clone(), method, app.policy.timeout_secs, app_state(app).into())
    }).collect();
    let mut preparations = BTreeMap::new();
    for app in archived.iter().chain(&apps) {
        if let Some(preparation) = status::preparation(app) { preparations.insert((app.unit.clone(), app.invocation_id.clone()), preparation); }
    }
    let (native, lifecycle) = lifecycle_capabilities(manager, &record).await;
    let mut details = base_details(status, &record, native, lifecycle);
    details.preparations = preparations.into_values().collect();
    details.capabilities.push(crate::window_capability::capability(manager, &record).await);
    if manager.actions.check().is_err() { return Ok(SessionStatusDetails::disabled()); }
    Ok(details)
}

fn app_state(app: &AppRecord) -> &'static str { if app.quit_started { "quitting" } else { "registered" } }

pub(super) async fn lifecycle_capabilities(manager: &UserManager, record: &SessionRecord) -> (Capability, Capability) {
    let native = native_logout(manager, record).await;
    let lifecycle = compositor_lifecycle(manager, record, &native).await;
    (native, lifecycle)
}

fn base_details(status: SessionStatus, record: &SessionRecord, native: Capability, lifecycle: Capability) -> SessionStatusDetails {
    let attached = record.provider == "partial";
    let partial = lifecycle.state != "available";
    let mut capabilities = vec![
        Capability::new("app-registration", "available", "Only explicitly registered application units are managed."),
        Capability::new("quit-command", "available", "Application-specific quit commands use the registered invocation's environment."),
        Capability::new("process-observation", "available", "Completion is observed through the unit and its cgroup."),
        Capability::new("term-kill", "available", "Signals require current invocation, cgroup and pinned process ownership."),
    ];
    capabilities.push(if status.xsmp_available {
        Capability::new("xsmp", "available", "The private XSMP server accepts authenticated registered application peers.")
    } else if matches!(record.provider.as_str(), "gnome" | "plasma") {
        Capability::new("xsmp", "not-applicable", "The native desktop retains its own session manager.")
    } else if !cfg!(feature = "xsmp") {
        Capability::new("xsmp", "unavailable", "This build has no libSM/libICE support.")
    } else { Capability::new("xsmp", "not-applicable", "This provider does not run a private XSMP server.") });
    let limitations = if attached {
        vec!["Only registered apps are prepared; compositor and PAM lifecycle are not owned. Non-native logout returns prepared and leaves the external WM running.".into()]
    } else if partial && matches!(record.provider.as_str(), "gnome" | "plasma") {
        vec![format!("Native desktop lifecycle control could not be verified: {}", native.reason)]
    } else if partial {
        vec![format!("Compositor lifecycle control could not be verified: {}", lifecycle.reason)]
    } else { Vec::new() };
    capabilities.push(lifecycle);
    capabilities.push(native);
    SessionStatusDetails { enabled: true, state: if partial { "partial" } else { "full" }.into(), status, capabilities,
        limitations,
        preparations: Vec::new() }
}

async fn compositor_lifecycle(manager: &UserManager, record: &SessionRecord, native: &Capability) -> Capability {
    let name = "compositor-lifecycle";
    if record.provider == "partial" {
        return Capability::new(name, "unavailable", "The external WM, shared targets and PAM login are not owned.");
    }
    if matches!(record.provider.as_str(), "gnome" | "plasma") {
        return Capability::new(name, &native.state, &native.reason);
    }
    let Some(unit) = &record.compositor_unit else {
        return if record.provider == "managed" {
            Capability::new(name, "unavailable", "No managed compositor unit is recorded.")
        } else { Capability::new(name, "available", "The session launcher controls the original session command.") };
    };
    if record.compositor_invocation.iter().all(|byte| *byte == 0) {
        return Capability::new(name, "unavailable", "No verified compositor invocation is recorded.");
    }
    match tokio::time::timeout(Duration::from_secs(2), current_compositor(manager, unit, &record.compositor_invocation)).await {
        Ok(Ok(true)) => Capability::new(name, "available", "The recorded compositor invocation is active and still owned."),
        Ok(Ok(false)) => Capability::new(name, "unavailable", "The recorded compositor invocation is missing, inactive or replaced."),
        Ok(Err(error)) => Capability::new(name, "unknown", &error.to_string()),
        Err(_) => Capability::new(name, "unknown", "The compositor lifecycle query timed out."),
    }
}

async fn current_compositor(manager: &UserManager, unit: &str, invocation: &[u8]) -> Result<bool, SessionError> {
    let query = async {
        if manager.invocation_id(unit).await? != invocation || !manager.active(unit).await? { return Ok(false); }
        Ok(manager.invocation_id(unit).await? == invocation)
    }.await;
    match query {
        Err(SessionError::Bus(error)) if crate::bus::missing_unit(&error) => Ok(false),
        result => result,
    }
}

pub(super) async fn native_logout(manager: &UserManager, record: &SessionRecord) -> Capability {
    let kind = match record.provider.as_str() {
        "gnome" => crate::provider::ProviderKind::Gnome, "plasma" => crate::provider::ProviderKind::Plasma,
        "partial" => crate::provider::ProviderKind::Attached,
        _ => return Capability::new("native-logout", "not-applicable", "This provider uses its own session lifecycle."),
    };
    let provider = crate::provider::Provider { kind, native_unit: None, logout_command: Vec::new() };
    let query = async {
        if kind == crate::provider::ProviderKind::Attached {
            crate::native_attached::discover(manager, &record.identity).await.map(|target| target.is_some())
        } else { provider.logout_available(manager, &record.identity).await }
    };
    match tokio::time::timeout(Duration::from_secs(2), query).await {
        Ok(Ok(true)) if kind == crate::provider::ProviderKind::Attached =>
            Capability::new("native-logout", "available", "The native logout peers belong to this logind session; the desktop owns confirmation dialogs."),
        Ok(Ok(true)) => Capability::new("native-logout", "available", "A verified native desktop has an owned or activatable logout service; it owns confirmation dialogs."),
        Ok(Ok(false)) => Capability::new("native-logout", "unavailable", "No live native desktop with an owned or activatable logout service was found."),
        Ok(Err(SessionError::Bus(zbus::Error::MethodError(name, reason, _)))) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied" =>
            Capability::new("native-logout", "unavailable", reason.as_deref().unwrap_or("Native desktop ownership could not be verified.")),
        Ok(Err(SessionError::Bus(zbus::Error::FDO(error)))) if matches!(error.as_ref(), zbus::fdo::Error::AccessDenied(_)) =>
            Capability::new("native-logout", "unavailable", &error.to_string()),
        Ok(Err(error)) => Capability::new("native-logout", "unknown", &error.to_string()),
        Err(_) => Capability::new("native-logout", "unknown", "The native desktop ownership query timed out."),
    }
}

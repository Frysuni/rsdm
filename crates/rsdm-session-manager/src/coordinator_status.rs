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
    let mut details = base_details(status, &record);
    details.preparations = preparations.into_values().collect();
    details.capabilities.push(native_logout(manager, &record).await);
    details.capabilities.push(crate::window_close::capability(manager, &record).await);
    if manager.actions.check().is_err() { return Ok(SessionStatusDetails::disabled()); }
    Ok(details)
}

fn app_state(app: &AppRecord) -> &'static str { if app.quit_started { "quitting" } else { "registered" } }

fn base_details(status: SessionStatus, record: &SessionRecord) -> SessionStatusDetails {
    let partial = record.provider == "partial";
    let mut capabilities = vec![
        Capability::new("app-registration", "available", "Only explicitly registered application units are managed."),
        Capability::new("quit-command", "available", "Application-specific quit commands use the registered invocation's environment."),
        Capability::new("process-observation", "available", "Completion is observed through the unit and its cgroup."),
        Capability::new("term-kill", "available", "Signals require current invocation, cgroup and pinned process ownership."),
    ];
    capabilities.push(if partial {
        Capability::new("compositor-lifecycle", "unavailable", "The external WM, shared targets and PAM login are not owned.")
    } else { Capability::new("compositor-lifecycle", "available", "The session launcher or native provider controls the session lifecycle.") });
    capabilities.push(if status.xsmp_available {
        Capability::new("xsmp", "available", "The private XSMP server accepts authenticated registered application peers.")
    } else if matches!(record.provider.as_str(), "gnome" | "plasma") {
        Capability::new("xsmp", "not-applicable", "The native desktop retains its own session manager.")
    } else if !cfg!(feature = "xsmp") {
        Capability::new("xsmp", "unavailable", "This build has no libSM/libICE support.")
    } else { Capability::new("xsmp", "not-applicable", "This provider does not run a private XSMP server.") });
    SessionStatusDetails { enabled: true, state: if partial { "partial" } else { "full" }.into(), status, capabilities,
        limitations: if partial { vec!["Only registered apps are prepared; compositor and PAM lifecycle are not owned. Non-native logout returns prepared and leaves the external WM running.".into()] } else { Vec::new() },
        preparations: Vec::new() }
}

async fn native_logout(manager: &UserManager, record: &SessionRecord) -> Capability {
    let name = match record.provider.as_str() {
        "gnome" => "org.gnome.SessionManager", "plasma" => "org.kde.LogoutPrompt",
        "partial" => return Capability::new("native-logout", "unavailable", "This attached provider currently prepares apps without native desktop delegation."),
        _ => return Capability::new("native-logout", "not-applicable", "This provider uses its own session lifecycle."),
    };
    let query = manager.read(|| async {
        let reply = manager.connection().call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "NameHasOwner", &(name,)).await?;
        reply.body().deserialize::<bool>()
    });
    match tokio::time::timeout(Duration::from_secs(2), query).await {
        Ok(Ok(true)) => Capability::new("native-logout", "available", "The native desktop logout service is present; it owns confirmation dialogs."),
        Ok(Ok(false)) => Capability::new("native-logout", "unavailable", "The native desktop logout service has no current bus owner."),
        Ok(Err(error)) => Capability::new("native-logout", "unknown", &error.to_string()),
        Err(_) => Capability::new("native-logout", "unknown", "The native desktop presence query timed out."),
    }
}

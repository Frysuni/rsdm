//! Registered applications use generic preparation when native delegation is unavailable.

use std::sync::{Arc, atomic::Ordering};

use rsdm_core::domain::{ShutdownMethod, TimeoutAction};

use super::{SessionError, app_stop::{self, ShutdownControl}, bus::UserManager, coordinator::{Coordinator, Work},
    runtime::Runtime, status::Capability};

pub(super) async fn prepare_applications(
    manager: &UserManager, runtime: &Runtime, control: Arc<ShutdownControl>,
    action: &str, native: Option<&Capability>,
) -> Result<Vec<String>, SessionError> {
    let native_available = native.is_some_and(|capability| capability.state == "available");
    let records = runtime.clone();
    let mut apps = super::runtime::blocking(move || records.apps()).await?;
    if native_available && !control.noncancelable.load(Ordering::SeqCst) {
        if apps.iter().any(|app| app.policy.method == ShutdownMethod::Auto
            && app.policy.on_timeout == TimeoutAction::Cancel && app.policy.quit_command.is_empty())
        {
            return Err(SessionError::State("native desktop shutdown cannot enforce the registered auto/cancel policy; use a quit command for an RSDM preflight".into()));
        }
        apps.retain(|app| app.policy.method != ShutdownMethod::Auto || !app.policy.quit_command.is_empty());
    }
    if !native_available && matches!(action, "poweroff" | "reboot") {
        manager.actions.guard(manager.deadline.bound(rsdm_system::logind::check(action))).await??;
    }
    app_stop::prepare_apps(manager, runtime, apps, control).await
}

impl Coordinator {
    pub(super) async fn prepare_native_remainder(&mut self) -> Result<(), SessionError> {
        self.action = "external-shutdown".into();
        self.lifecycle.phase = rsdm_core::domain::SessionPhase::Preparing;
        self.preparing = true;
        self.save().await?;
        let manager = self.manager.clone();
        let runtime = self.runtime.clone();
        let control = self.shutdown.as_ref().expect("shutdown control").clone();
        let events = self.events.clone();
        self.workers += 1;
        tokio::spawn(async move {
            let result = prepare_remaining_apps(&manager, &runtime, control).await;
            let _ = events.send(Work::Prepared(result, None));
        });
        Ok(())
    }
}

async fn prepare_remaining_apps(manager: &UserManager, runtime: &Runtime, control: Arc<ShutdownControl>) -> Result<Vec<String>, SessionError> {
    let records = runtime.clone();
    let mut apps = super::runtime::blocking(move || records.apps()).await?;
    apps.retain(|app| app.policy.method == ShutdownMethod::Auto && app.policy.quit_command.is_empty());
    app_stop::prepare_apps(manager, runtime, apps, control).await
}

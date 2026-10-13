//! Registered applications use generic preparation when native delegation is unavailable.

use std::sync::{Arc, atomic::Ordering};

use rsdm_core::domain::{ShutdownMethod, TimeoutAction};

use super::{SessionError, app_stop::{self, ShutdownControl}, bus::UserManager,
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

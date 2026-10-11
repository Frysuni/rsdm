//! Graceful method selection and quit command service dispatch.

use std::{path::Path, sync::atomic::Ordering, time::Duration};

use rsdm_core::domain::ShutdownMethod;
use zbus::zvariant::Value;

use super::ShutdownControl;
use crate::{SessionError, bus::UserManager, processes::{app_processes, terminate_main},
    runtime::{AppRecord, Runtime}, units::command_properties};

pub(super) async fn request_quit(manager: &UserManager, runtime: &Runtime, app: &AppRecord, control: &ShutdownControl) -> Result<Option<String>, SessionError> {
    if !app.policy.quit_command.is_empty() {
        let anchor = quit_anchor(manager, runtime).await?;
        if !crate::app_dispatch::claim(runtime, app, control.transaction_id.as_deref(), crate::app_dispatch::Method::Command, &manager.deadline).await? {
            return Ok(None);
        }
        let dispatched = launch_quit(manager, app, &anchor).await;
        let state = if dispatched.is_ok() { crate::app_dispatch::State::Sent }
            else if matches!(&dispatched, Err(SessionError::StartRejected { reference_retained: false, .. })) { crate::app_dispatch::State::NotSent }
            else { crate::app_dispatch::State::Intent };
        crate::app_dispatch::finish(runtime, app, control.transaction_id.as_deref(), crate::app_dispatch::Method::Command, state, &manager.deadline).await?;
        return match dispatched {
            Ok(unit) => Ok(Some(unit)),
            Err(error @ SessionError::StartRejected { .. }) => Err(error),
            Err(error) => { tracing::warn!(%error, unit = app.unit, "quit command dispatch is unconfirmed; observing the application"); Ok(None) },
        };
    }
    if app.policy.method != ShutdownMethod::Term {
        let selected = match control.xsmp_units.get() {
            Some(selected) => selected.clone(),
            None => prepare_xsmp(manager, runtime, std::slice::from_ref(&app.unit), control).await?,
        };
        if selected.contains(&app.unit) {
            let _ = crate::app_dispatch::claim(runtime, app, control.transaction_id.as_deref(), crate::app_dispatch::Method::Xsmp, &manager.deadline).await?;
            return Ok(None);
        }
        if app.policy.method == ShutdownMethod::Xsmp && !control.noncancelable.load(Ordering::SeqCst)
            && !app_processes(manager, app).await?.is_empty()
        {
            return Err(SessionError::State("this application has no XSMP connection".into()));
        }
    }
    if app.policy.method == ShutdownMethod::Auto && !control.cancelled()
        && crate::window_close::request(manager, runtime, app, control).await?
    {
        return Ok(None);
    }
    if crate::app_dispatch::claim(runtime, app, control.transaction_id.as_deref(), crate::app_dispatch::Method::Term, &manager.deadline).await? {
        terminate_main(manager, app).await?;
        crate::app_dispatch::finish(runtime, app, control.transaction_id.as_deref(), crate::app_dispatch::Method::Term, crate::app_dispatch::State::Sent, &manager.deadline).await?;
    }
    Ok(None)
}

pub(super) async fn prepare_xsmp(
    manager: &UserManager, runtime: &Runtime, units: &[String], control: &ShutdownControl,
) -> Result<Vec<String>, SessionError> {
    if !cfg!(feature = "xsmp") || units.is_empty() || control.recovery.load(Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    manager.actions.guard(manager.deadline.bound(async {
        let connection = manager.connection();
        let proxy = zbus::Proxy::new(&connection, crate::control::BUS_NAME,
            crate::control::OBJECT_PATH, crate::control::BUS_NAME).await?;
        let selected = proxy.call("XsmpPrepare", &(&runtime.generation, control.transaction_id.as_deref().unwrap_or(""), units, !control.noncancelable.load(Ordering::SeqCst))).await?;
        Ok::<Vec<String>, SessionError>(selected)
    })).await?
}

async fn quit_anchor(manager: &UserManager, runtime: &Runtime) -> Result<String, SessionError> {
    let records = runtime.clone();
    let record = crate::runtime::blocking(move || records.session()).await?;
    if record.disabled { return Err(SessionError::Disabled); }
    if record.provider != "partial" { return Ok(record.anchor_unit); }
    let (unit, invocation) = record.coordinator_unit
        .ok_or_else(|| SessionError::State("attached coordinator ownership is missing".into()))?;
    if unit != crate::attach::UNIT || invocation.is_empty() || invocation.iter().all(|byte| *byte == 0)
        || manager.invocation_id(&unit).await? != invocation
    {
        return Err(SessionError::State("attached coordinator invocation changed before quit dispatch".into()));
    }
    Ok(unit)
}

pub(super) async fn launch_quit(manager: &UserManager, app: &AppRecord, anchor: &str) -> Result<String, SessionError> {
    let unit = app.unit.replacen("app-rsdm-", "rsdm-quit-", 1);
    let environment: Vec<String> = manager.unit_property(&app.unit, "org.freedesktop.systemd1.Service", "Environment").await?;
    let environment: Vec<_> = environment.iter().filter_map(|entry| entry.split_once('=')
        .map(|(name, value)| (name.to_string(), value.to_string()))).collect();
    let directory: String = manager.unit_property(&app.unit, "org.freedesktop.systemd1.Service", "WorkingDirectory").await?;
    let mut properties = command_properties(&app.policy.quit_command, &environment, Path::new(&directory)).await?;
    properties.extend([
        ("Slice", Value::from(crate::units::APP_SLICE)),
        ("TimeoutStopUSec", Value::from(1_000_000_u64)),
        ("PartOf", Value::new(vec![anchor.to_string()])),
        ("Requisite", Value::new(vec![anchor.to_string()])),
    ]);
    if let Err(error) = manager.start_service(&unit, &properties).await {
        if !matches!(error, SessionError::StartRejected { .. }) {
            release_quit(manager, &unit).await;
        }
        return Err(error);
    }
    Ok(unit)
}

pub(super) async fn release_quit(manager: &UserManager, unit: &str) {
    if let Err(error) = manager.stop(unit, Duration::from_secs(3)).await {
        tracing::warn!(unit, %error, "quit command cleanup failed");
    }
    let _ = manager.unref(unit).await;
}

#[cfg(test)]
#[path = "app_stop_quit_tests.rs"]
mod tests;

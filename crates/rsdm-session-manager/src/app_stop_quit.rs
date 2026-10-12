//! Graceful method selection and quit command service dispatch.

use std::{path::Path, sync::atomic::{AtomicBool, Ordering}, time::Duration};

use rsdm_core::domain::ShutdownMethod;
use zbus::zvariant::Value;

use super::ShutdownControl;
use crate::{SessionError, bus::UserManager, processes::{app_processes, terminate_main},
    runtime::{AppRecord, Runtime}, units::command_properties};

pub(super) async fn request_quit(manager: &UserManager, runtime: &Runtime, app: &AppRecord, control: &ShutdownControl) -> Result<Option<String>, SessionError> {
    if control.cancelled() { return Ok(None); }
    if !app.policy.quit_command.is_empty() {
        return request_command(manager, runtime, app, control).await;
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
    request_term(manager, runtime, app, control).await?;
    Ok(None)
}

async fn request_command(manager: &UserManager, runtime: &Runtime, app: &AppRecord, control: &ShutdownControl) -> Result<Option<String>, SessionError> {
    let anchor = quit_anchor(manager, runtime).await?;
    let transaction = control.transaction_id.as_deref();
    if !crate::app_dispatch::claim(runtime, app, transaction, crate::app_dispatch::Method::Command, &manager.deadline).await? {
        return Ok(None);
    }

    let dispatched = launch_quit(manager, app, &anchor, control).await;
    let state = if dispatched.is_ok() { crate::app_dispatch::State::Sent }
        else if matches!(&dispatched, Err(SessionError::StartRejected { reference_retained: false, .. })) { crate::app_dispatch::State::NotSent }
        else { crate::app_dispatch::State::Intent };
    crate::app_dispatch::finish(runtime, app, transaction, crate::app_dispatch::Method::Command, state, &manager.deadline).await?;
    match dispatched {
        Ok(unit) => Ok(Some(unit)),
        Err(SessionError::StartRejected { reference_retained: false, .. }) if control.cancelled() => Ok(None),
        Err(error @ SessionError::StartRejected { .. }) => Err(error),
        Err(error) => { tracing::warn!(%error, unit = app.unit, "quit command dispatch is unconfirmed; observing the application"); Ok(None) },
    }
}

async fn request_term(manager: &UserManager, runtime: &Runtime, app: &AppRecord, control: &ShutdownControl) -> Result<(), SessionError> {
    if control.cancelled() { return Ok(()); }
    let transaction = control.transaction_id.as_deref();
    if !crate::app_dispatch::claim(runtime, app, transaction, crate::app_dispatch::Method::Term, &manager.deadline).await? {
        return Ok(());
    }

    let sent = terminate_main(manager, app, control).await?;
    let state = if sent { crate::app_dispatch::State::Sent } else { crate::app_dispatch::State::NotSent };
    crate::app_dispatch::finish(runtime, app, transaction, crate::app_dispatch::Method::Term, state, &manager.deadline).await
}

pub(super) async fn prepare_xsmp(
    manager: &UserManager, runtime: &Runtime, units: &[String], control: &ShutdownControl,
) -> Result<Vec<String>, SessionError> {
    if !cfg!(feature = "xsmp") || units.is_empty() || control.recovery.load(Ordering::SeqCst) || control.cancelled() {
        return Ok(Vec::new());
    }
    manager.actions.guard(manager.deadline.bound(async {
        let connection = manager.connection();
        let proxy = zbus::Proxy::new(&connection, crate::control::BUS_NAME,
            crate::control::OBJECT_PATH, crate::control::BUS_NAME).await?;
        if control.cancelled() { return Ok(Vec::new()); }
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

pub(super) async fn launch_quit(manager: &UserManager, app: &AppRecord, anchor: &str, control: &ShutdownControl) -> Result<String, SessionError> {
    let unit = app.unit.replacen("app-rsdm-", "rsdm-quit-", 1);
    let submitted = AtomicBool::new(false);
    let result = async {
        let properties = quit_properties(manager, app, anchor).await?;
        manager.start_service_tracked(&unit, &properties, &submitted, Some((control, app))).await
    }.await;
    if let Err(error) = result {
        if !submitted.load(Ordering::Relaxed) && !matches!(error, SessionError::StartRejected { .. }) {
            return Err(SessionError::StartRejected { error: Box::new(error), reference_retained: false });
        }
        if !matches!(error, SessionError::StartRejected { .. }) {
            release_quit(manager, &unit).await;
        }
        return Err(error);
    }
    Ok(unit)
}

async fn quit_properties(manager: &UserManager, app: &AppRecord, anchor: &str) -> Result<crate::bus::UnitProperties, SessionError> {
    if !crate::processes::verify_invocation(manager, app).await? {
        return Err(SessionError::State("application exited before quit command preparation".into()));
    }
    let environment: Vec<String> = manager.unit_property(&app.unit, "org.freedesktop.systemd1.Service", "Environment").await?;
    let environment: Vec<_> = environment.iter().filter_map(|entry| entry.split_once('=')
        .map(|(name, value)| (name.to_string(), value.to_string()))).collect();
    let directory: String = manager.unit_property(&app.unit, "org.freedesktop.systemd1.Service", "WorkingDirectory").await?;
    let mut properties = command_properties(&app.policy.quit_command, &environment, Path::new(&directory)).await?;
    if !crate::processes::verify_invocation(manager, app).await? {
        return Err(SessionError::State("application exited during quit command preparation".into()));
    }
    properties.extend([
        ("Slice", Value::from(crate::units::APP_SLICE)),
        ("TimeoutStopUSec", Value::from(1_000_000_u64)),
        ("PartOf", Value::new(vec![anchor.to_string()])),
        ("Requisite", Value::new(vec![anchor.to_string()])),
    ]);
    Ok(properties)
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

#[cfg(test)]
#[path = "app_stop_signal_tests.rs"]
mod signal_tests;

#[cfg(test)]
#[path = "app_stop_command_tests.rs"]
mod command_tests;

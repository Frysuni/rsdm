//! Application shutdown, shared by preflight and the synchronous ExecStop hook.

use std::{
    path::Path,
    sync::{Arc, OnceLock, atomic::{AtomicBool, Ordering}},
    thread,
    time::Duration,
};

use rsdm_core::domain::ShutdownMethod;
use zbus::zvariant::Value;

use super::{
    SessionError, bus::UserManager, deadline::Deadline,
    processes::{app_processes, monotonic_usec, terminate_main},
    runtime::{AppRecord, Runtime}, units::command_properties,
};

#[path = "app_stop_wait.rs"]
mod wait;

#[derive(Default)]
pub(super) struct ShutdownControl {
    pub cancelled: AtomicBool,
    pub noncancelable: AtomicBool,
    pub hard_deadline: Deadline,
    pub recovery: AtomicBool,
    xsmp_units: OnceLock<Vec<String>>,
}

impl ShutdownControl {
    pub fn for_manager(manager: &UserManager) -> Self {
        Self { hard_deadline: manager.deadline.clone(), ..Self::default() }
    }

    pub fn force(&self, deadline: u64) {
        self.noncancelable.store(true, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        self.hard_deadline.set(deadline);
    }

    fn cancelled(&self) -> bool {
        !self.noncancelable.load(Ordering::SeqCst) && self.cancelled.load(Ordering::SeqCst)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum AppOutcome {
    Closed,
    Cancelled,
    Forced,
}

pub(super) fn prepare_apps(
    manager: &UserManager, runtime: &Runtime, apps: Vec<AppRecord>, control: Arc<ShutdownControl>,
) -> Result<Vec<String>, SessionError> {
    let phase_manager = preparation_manager(manager, &control)?;
    let selected = prepare_xsmp(
        &phase_manager, runtime, &apps.iter().map(|app| app.unit.clone()).collect::<Vec<_>>(), &control,
    )?;
    // Keep one protocol selection for the batch. Reissuing Prepare from each
    // app worker could start a new save after the server already cancelled it.
    let _ = control.xsmp_units.set(selected);
    let workers: Vec<_> = apps.into_iter().map(|app| {
        let manager = manager.clone();
        let runtime = runtime.clone();
        let control = control.clone();
        thread::spawn(move || {
            let result = prepare_app(&manager, &runtime, &app.unit, &control);
            (app.unit, result)
        })
    }).collect();
    let mut forced = Vec::new();
    let mut failure = None;
    for worker in workers {
        match worker.join() {
            Ok((unit, Ok(AppOutcome::Forced))) => forced.push(unit),
            Ok((_, Ok(_))) => {}
            Ok((_, Err(error))) => {
                control.cancelled.store(true, Ordering::SeqCst);
                failure.get_or_insert(error);
            }
            Err(_) => {
                failure.get_or_insert(SessionError::State("application shutdown worker panicked".into()));
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(forced)
}

pub(super) fn prepare_app(
    manager: &UserManager, runtime: &Runtime, unit: &str, control: &ShutdownControl,
) -> Result<AppOutcome, SessionError> {
    let phase_manager = preparation_manager(manager, control)?;
    let prepared = begin_preparation(&phase_manager, runtime, unit, control);
    let (app, quit_unit) = match prepared {
        Ok(Preparation::Finished(outcome)) => return Ok(outcome),
        Ok(Preparation::Waiting(app, quit_unit)) => (app, quit_unit),
        Err(_) if phase_manager.deadline.remaining(Duration::MAX).is_err()
            && control.noncancelable.load(Ordering::SeqCst) => {
                let mut force_manager = manager.clone();
                force_manager.deadline = control.hard_deadline.reserving(Duration::from_secs(1))?;
                if !super::apps::pin_pending(&force_manager, runtime, unit)? { return Ok(AppOutcome::Closed); }
                return wait::force_exit(&force_manager, &runtime.app(unit)?, control, &phase_manager.deadline);
            }
        Err(error) => return Err(error),
    };
    let result = wait::await_exit(&phase_manager, &app, control);
    if let Some(unit) = quit_unit {
        let mut cleanup_manager = manager.clone();
        cleanup_manager.deadline = control.hard_deadline.reserving(Duration::from_secs(1))?;
        release_quit(&cleanup_manager, &unit);
    }
    if matches!(result, Ok(AppOutcome::Cancelled)) {
        reset_preparation(runtime, &app, &control.hard_deadline)?;
    }
    result
}

enum Preparation {
    Finished(AppOutcome),
    Waiting(AppRecord, Option<String>),
}

fn preparation_manager(manager: &UserManager, control: &ShutdownControl) -> Result<UserManager, SessionError> {
    let mut manager = manager.clone();
    manager.deadline = control.hard_deadline.reserving(Duration::from_secs(2))?;
    Ok(manager)
}

fn begin_preparation(
    manager: &UserManager, runtime: &Runtime, unit: &str, control: &ShutdownControl,
) -> Result<Preparation, SessionError> {
    if !super::apps::pin_pending(manager, runtime, unit)? {
        return Ok(Preparation::Finished(AppOutcome::Closed));
    }
    let app = runtime.app(unit)?;
    if app_processes(manager, &app)?.is_empty() {
        return Ok(Preparation::Finished(AppOutcome::Closed));
    }
    if control.cancelled() {
        return Ok(Preparation::Finished(AppOutcome::Cancelled));
    }

    let (app, claimed) = claim_preparation(runtime, unit, control, &manager.deadline)?;
    let quit_unit = if claimed { request_quit(manager, runtime, &app, control)? } else { None };
    Ok(Preparation::Waiting(app, quit_unit))
}

fn claim_preparation(runtime: &Runtime, unit: &str, control: &ShutdownControl, phase_deadline: &Deadline) -> Result<(AppRecord, bool), SessionError> {
    let _lease = runtime.app_lease_until(unit, phase_deadline)?;
    let mut app = runtime.app(unit)?;
    let deadline = app.deadline_usec.get_or_insert(
        monotonic_usec()?.checked_add(app.policy.timeout_secs * 1_000_000)
            .ok_or_else(|| SessionError::State("shutdown deadline overflow".into()))?
    );
    let hard = control.hard_deadline.get();
    if hard != 0 {
        *deadline = (*deadline).min(hard);
    }
    let claimed = !app.quit_started;
    app.quit_started = true;
    runtime.save_app(&app)?;
    Ok((app, claimed))
}

fn reset_preparation(runtime: &Runtime, prepared: &AppRecord, deadline: &Deadline) -> Result<(), SessionError> {
    let _lease = runtime.app_lease_until(&prepared.unit, deadline)?;
    let mut app = runtime.app(&prepared.unit)?;
    if app.invocation_id == prepared.invocation_id && app.deadline_usec == prepared.deadline_usec {
        app.deadline_usec = None;
        app.quit_started = false;
        runtime.save_app(&app)?;
    }
    Ok(())
}

fn request_quit(manager: &UserManager, runtime: &Runtime, app: &AppRecord, control: &ShutdownControl) -> Result<Option<String>, SessionError> {
    if !app.policy.quit_command.is_empty() {
        return launch_quit(manager, app, &runtime.generation).map(Some);
    }
    if app.policy.method != ShutdownMethod::Term {
        let selected = match control.xsmp_units.get() {
            Some(selected) => selected.clone(),
            None => prepare_xsmp(manager, runtime, std::slice::from_ref(&app.unit), control)?,
        };
        if selected.contains(&app.unit) {
            return Ok(None);
        }
        if app.policy.method == ShutdownMethod::Xsmp && !control.noncancelable.load(Ordering::SeqCst)
            && !app_processes(manager, app)?.is_empty()
        {
            return Err(SessionError::State("this application has no XSMP connection".into()));
        }
    }
    terminate_main(manager, app)?;
    Ok(None)
}

fn prepare_xsmp(
    manager: &UserManager, runtime: &Runtime, units: &[String], control: &ShutdownControl,
) -> Result<Vec<String>, SessionError> {
    if !cfg!(feature = "xsmp") || units.is_empty() || control.recovery.load(Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    let result = async_io::block_on(manager.deadline.bound(async {
        let connection = manager.connection();
        let proxy = zbus::Proxy::new(&connection, super::control::BUS_NAME,
            super::control::OBJECT_PATH, super::control::BUS_NAME).await?;
        let selected = proxy.call("XsmpPrepare", &(&runtime.generation, units, !control.noncancelable.load(Ordering::SeqCst))).await?;
        Ok::<Vec<String>, SessionError>(selected)
    }));
    match result {
        Err(error) if control.noncancelable.load(Ordering::SeqCst) => {
            tracing::warn!(%error, "XSMP is unavailable during noncancellable cleanup; using application signals");
            Ok(Vec::new())
        }
        other => other,
    }
}

fn launch_quit(manager: &UserManager, app: &AppRecord, generation: &str) -> Result<String, SessionError> {
    let unit = app.unit.replacen("app-rsdm-", "rsdm-quit-", 1);
    let environment: Vec<String> = manager.unit_property(&app.unit, "org.freedesktop.systemd1.Service", "Environment")?;
    let environment: Vec<_> = environment.iter().filter_map(|entry| entry.split_once('=')
        .map(|(name, value)| (name.to_string(), value.to_string()))).collect();
    let directory: String = manager.unit_property(&app.unit, "org.freedesktop.systemd1.Service", "WorkingDirectory")?;
    let mut properties = command_properties(&app.policy.quit_command, &environment, Path::new(&directory))?;
    properties.extend([
        ("Slice", Value::from(super::units::APP_SLICE)),
        ("TimeoutStopUSec", Value::from(1_000_000_u64)),
        ("PartOf", Value::new(vec![format!("rsdm-session-{generation}.service")])),
        ("Requisite", Value::new(vec![format!("rsdm-session-{generation}.service")])),
    ]);
    if let Err(error) = manager.start_service(&unit, &properties) {
        if !matches!(error, SessionError::StartRejected { .. }) {
            release_quit(manager, &unit);
        }
        return Err(error);
    }
    Ok(unit)
}

fn release_quit(manager: &UserManager, unit: &str) {
    if let Err(error) = manager.stop(unit, Duration::from_secs(3)) {
        tracing::warn!(unit, %error, "quit command cleanup failed");
    }
    let _ = manager.unref(unit);
}

#[cfg(test)]
#[path = "app_stop_tests.rs"]
mod tests;

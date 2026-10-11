//! Application shutdown, shared by preflight and the ExecStop hook.

use std::{
    sync::{Arc, OnceLock, atomic::{AtomicBool, Ordering}},
    time::Duration,
};

use super::{
    SessionError, bus::UserManager, deadline::Deadline,
    processes::{app_processes, monotonic_usec},
    runtime::{blocking, AppRecord, Runtime},
};

#[path = "app_stop_wait.rs"]
mod wait;

#[path = "app_stop_quit.rs"]
mod quit;
use quit::{request_quit, prepare_xsmp, release_quit};
#[cfg(test)]
use quit::launch_quit;

#[derive(Default)]
pub(super) struct ShutdownControl {
    pub cancelled: AtomicBool,
    pub noncancelable: AtomicBool,
    pub hard_deadline: Deadline,
    pub recovery: AtomicBool,
    xsmp_units: OnceLock<Vec<String>>,
    pub(super) transaction_id: Option<String>,
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

pub(super) async fn prepare_apps(
    manager: &UserManager, runtime: &Runtime, apps: Vec<AppRecord>, control: Arc<ShutdownControl>,
) -> Result<Vec<String>, SessionError> {
    let phase_manager = preparation_manager(manager, &control)?;
    let selected = prepare_xsmp(
        &phase_manager, runtime, &apps.iter().map(|app| app.unit.clone()).collect::<Vec<_>>(), &control,
    ).await?;
    // Keep one protocol selection for the batch. Reissuing Prepare from each
    // app worker could start a new save after the server already cancelled it.
    let _ = control.xsmp_units.set(selected);

    let mut workers = tokio::task::JoinSet::new();
    for app in apps {
        let manager = manager.clone();
        let runtime = runtime.clone();
        let control = control.clone();
        workers.spawn(async move {
            let result = prepare_app(&manager, &runtime, &app.unit, &control).await;
            (app.unit, result)
        });
    }

    let mut forced = Vec::new();
    let mut failure = None;
    while let Some(result) = workers.join_next().await {
        match result {
            Ok((unit, Ok(AppOutcome::Forced))) => forced.push(unit),
            Ok((_, Ok(_))) => {}
            Ok((_, Err(error))) => {
                control.cancelled.store(true, Ordering::SeqCst);
                failure.get_or_insert(error);
            }
            Err(error) => {
                control.cancelled.store(true, Ordering::SeqCst);
                failure.get_or_insert(SessionError::State(format!("application shutdown worker failed: {error}")));
            }
        }
    }

    if let Some(error) = failure {
        return Err(error);
    }
    Ok(forced)
}

pub(super) async fn prepare_app(
    manager: &UserManager, runtime: &Runtime, unit: &str, control: &ShutdownControl,
) -> Result<AppOutcome, SessionError> {
    let phase_manager = preparation_manager(manager, control)?;
    let prepared = begin_preparation(&phase_manager, runtime, unit, control).await;
    let (app, quit_unit) = match prepared {
        Ok(Preparation::Finished(outcome)) => return Ok(outcome),
        Ok(Preparation::Waiting(app, quit_unit)) => (app, quit_unit),
        Err(_) if phase_manager.deadline.remaining(Duration::MAX).is_err()
            && control.noncancelable.load(Ordering::SeqCst) => {
                let mut force_manager = manager.clone();
                force_manager.deadline = control.hard_deadline.reserving(Duration::from_secs(1))?;
                if !super::apps::pin_pending(&force_manager, runtime, unit).await? { return Ok(AppOutcome::Closed); }
                let records = runtime.clone();
                let unit = unit.to_string();
                let app = blocking(move || records.app(&unit)).await?;
                return wait::force_exit(&force_manager, &app, control, &phase_manager.deadline).await;
            }
        Err(error) => return Err(error),
    };
    let result = wait::await_exit(&phase_manager, &app, control).await;
    if let Some(unit) = quit_unit {
        let mut cleanup_manager = manager.clone();
        cleanup_manager.deadline = control.hard_deadline.reserving(Duration::from_secs(1))?;
        release_quit(&cleanup_manager, &unit).await;
    }
    if matches!(result, Ok(AppOutcome::Cancelled)) {
        reset_preparation(runtime, &app, &control.hard_deadline).await?;
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

async fn begin_preparation(
    manager: &UserManager, runtime: &Runtime, unit: &str, control: &ShutdownControl,
) -> Result<Preparation, SessionError> {
    if !super::apps::pin_pending(manager, runtime, unit).await? {
        return Ok(Preparation::Finished(AppOutcome::Closed));
    }
    let records = runtime.clone();
    let app_unit = unit.to_string();
    let app = blocking(move || records.app(&app_unit)).await?;
    if app_processes(manager, &app).await?.is_empty() {
        return Ok(Preparation::Finished(AppOutcome::Closed));
    }
    if control.cancelled() {
        return Ok(Preparation::Finished(AppOutcome::Cancelled));
    }

    let (app, claimed) = claim_preparation(runtime, unit, control, &manager.deadline).await?;
    if control.cancelled() {
        if claimed { reset_preparation(runtime, &app, &control.hard_deadline).await?; }
        return Ok(Preparation::Finished(AppOutcome::Cancelled));
    }
    let quit_unit = if claimed { request_quit(manager, runtime, &app, control).await? } else { None };
    Ok(Preparation::Waiting(app, quit_unit))
}

async fn claim_preparation(runtime: &Runtime, unit: &str, control: &ShutdownControl, phase_deadline: &Deadline) -> Result<(AppRecord, bool), SessionError> {
    let runtime = runtime.clone();
    let unit = unit.to_string();
    let phase_deadline = phase_deadline.clone();
    let hard_deadline = control.hard_deadline.clone();
    blocking(move || {
        let _lease = runtime.app_lease_until(&unit, &phase_deadline)?;
        let mut app = runtime.app(&unit)?;
        let claimed = app.deadline_usec.is_none() && !app.quit_started;
        let deadline = app.deadline_usec.get_or_insert(
            monotonic_usec()?.checked_add(app.policy.timeout_secs * 1_000_000)
                .ok_or_else(|| SessionError::State("shutdown deadline overflow".into()))?
        );
        let hard = hard_deadline.get();
        if hard != 0 { *deadline = (*deadline).min(hard); }
        runtime.save_app(&app)?;
        Ok((app, claimed))
    }).await
}

async fn reset_preparation(runtime: &Runtime, prepared: &AppRecord, deadline: &Deadline) -> Result<(), SessionError> {
    let runtime = runtime.clone();
    let prepared = prepared.clone();
    let deadline = deadline.clone();
    blocking(move || {
        let _lease = runtime.app_lease_until(&prepared.unit, &deadline)?;
        let mut app = runtime.app(&prepared.unit)?;
        if app.invocation_id == prepared.invocation_id && app.deadline_usec == prepared.deadline_usec {
            app.deadline_usec = None;
            super::app_dispatch::cancel(&mut app);
            runtime.save_app(&app)?;
        }
        Ok(())
    }).await
}

#[cfg(test)]
#[path = "app_stop_tests.rs"]
mod tests;

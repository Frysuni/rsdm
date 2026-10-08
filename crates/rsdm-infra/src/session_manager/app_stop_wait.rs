//! Graceful waiting and escalation leave time for infrastructure teardown.

use rsdm_core::domain::TimeoutAction;
use std::{sync::atomic::Ordering, thread, time::Duration};

use super::{AppOutcome, ShutdownControl};
use crate::session_manager::{
    SessionError, bus::UserManager, deadline::Deadline,
    processes::{app_processes, monotonic_usec, signal_app}, runtime::AppRecord,
};

pub(super) fn await_exit(
    manager: &UserManager, app: &AppRecord, control: &ShutdownControl,
) -> Result<AppOutcome, SessionError> {
    let deadline = app.deadline_usec.expect("prepared deadline");
    if let Some(outcome) = wait_until(manager, app, deadline, control)? { return Ok(outcome); }
    if app.policy.on_timeout == TimeoutAction::Cancel && !control.noncancelable.load(Ordering::SeqCst) {
        control.cancelled.store(true, Ordering::SeqCst);
        return Ok(AppOutcome::Cancelled);
    }
    force_exit(manager, app, control, &manager.deadline)
}

pub(super) fn force_exit(
    manager: &UserManager, app: &AppRecord, control: &ShutdownControl, grace: &Deadline,
) -> Result<AppOutcome, SessionError> {
    if control.cancelled() { return Ok(AppOutcome::Cancelled); }
    let mut force_manager = manager.clone();
    force_manager.deadline = control.hard_deadline.reserving(Duration::from_secs(1))?;
    if app_processes(&force_manager, app)?.is_empty() { return Ok(AppOutcome::Closed); }

    signal_app(&force_manager, app, libc::SIGTERM)?;
    let mut grace_manager = force_manager.clone();
    grace_manager.deadline = grace.clone();
    if let Some(outcome) = wait_until(&grace_manager, app, monotonic_usec()?.saturating_add(5_000_000), control)? {
        return Ok(if outcome == AppOutcome::Closed { AppOutcome::Forced } else { outcome });
    }
    if control.cancelled() { return Ok(AppOutcome::Cancelled); }
    signal_app(&force_manager, app, libc::SIGKILL)?;
    match wait_until(&force_manager, app, monotonic_usec()?.saturating_add(5_000_000), control)? {
        Some(AppOutcome::Closed) => Ok(AppOutcome::Forced),
        Some(outcome) => Ok(outcome),
        None => Err(SessionError::State(format!("{} still has processes after SIGKILL", app.unit))),
    }
}

fn wait_until(
    manager: &UserManager, app: &AppRecord, deadline: u64, control: &ShutdownControl,
) -> Result<Option<AppOutcome>, SessionError> {
    let mut poll_delay = Duration::from_millis(20);
    loop {
        if control.cancelled() { return Ok(Some(AppOutcome::Cancelled)); }
        let effective = match manager.deadline.get() {
            0 => deadline,
            hard => deadline.min(hard),
        };
        let remaining = effective.saturating_sub(monotonic_usec()?);
        if remaining == 0 { return Ok(None); }
        match app_processes(manager, app) {
            Ok(processes) if processes.is_empty() => return Ok(Some(AppOutcome::Closed)),
            Ok(_) => {}
            Err(_) if manager.deadline.remaining(Duration::MAX).is_err() => return Ok(None),
            Err(error) => return Err(error),
        }
        let sleep = Duration::from_micros(deadline.saturating_sub(monotonic_usec()?)).min(poll_delay);
        if let Ok(sleep) = manager.deadline.remaining(sleep) { thread::sleep(sleep); }
        poll_delay = poll_delay.saturating_mul(2).min(Duration::from_millis(250));
    }
}

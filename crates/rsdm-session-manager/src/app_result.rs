//! Preparation outcomes stay bound to their application invocation.

use serde::{Deserialize, Serialize};

use crate::{SessionError, deadline::Deadline, runtime::{AppRecord, Runtime, blocking}};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome { Clean, Forced, Cancelled, Unknown }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preparation {
    pub(super) transaction_id: Option<String>,
    pub(super) outcome: Outcome,
    pub(super) escalated: bool,
    // Absence identifies legacy flags that recorded intent, not acceptance.
    // A pending attempt survives recovery and overlapping stop hooks.
    #[serde(default)]
    pub(super) escalation_pending: Option<u32>,
}

impl Preparation {
    pub(super) fn confirmed_escalation(&self) -> bool {
        self.escalated && self.escalation_pending.is_some()
    }

    pub(super) fn uncertain_escalation(&self) -> bool {
        self.pending_escalations() > 0
    }

    fn pending_escalations(&self) -> u32 {
        self.escalation_pending.unwrap_or(u32::from(self.escalated))
    }
}

pub(super) fn start(app: &mut AppRecord, transaction_id: Option<&str>) {
    let escalated = app.preparation.as_ref().is_some_and(Preparation::confirmed_escalation);
    let pending = app.preparation.as_ref().map_or(0, Preparation::pending_escalations);
    app.preparation = Some(Preparation { transaction_id: transaction_id.map(str::to_string), outcome: Outcome::Unknown,
        escalated, escalation_pending: Some(pending) });
}

pub(super) async fn begin(runtime: &Runtime, prepared: &AppRecord, transaction_id: Option<&str>, deadline: &Deadline) -> Result<(), SessionError> {
    let runtime = runtime.clone();
    let prepared = prepared.clone();
    let transaction_id = transaction_id.map(str::to_string);
    let deadline = deadline.clone();
    blocking(move || {
        let _lease = runtime.app_lease_until(&prepared.unit, &deadline)?;
        let mut app = runtime.app(&prepared.unit)?;
        if app.invocation_id != prepared.invocation_id { return Err(SessionError::State("application invocation changed before preparation tracking".into())); }
        start(&mut app, transaction_id.as_deref());
        runtime.save_app(&app)
    }).await
}

pub(super) async fn escalating(runtime: &Runtime, prepared: &AppRecord, transaction_id: Option<&str>, deadline: &Deadline) -> Result<(), SessionError> {
    let runtime = runtime.clone();
    let prepared = prepared.clone();
    let transaction_id = transaction_id.map(str::to_string);
    let deadline = deadline.clone();
    blocking(move || {
        let _lease = runtime.app_lease_until(&prepared.unit, &deadline)?;
        let mut app = runtime.app(&prepared.unit)?;
        if app.invocation_id != prepared.invocation_id || app.invocation_id.is_empty() {
            return Err(SessionError::State("application invocation changed before escalation tracking".into()));
        }
        if app.preparation.is_none() { start(&mut app, transaction_id.as_deref()); }
        let preparation = app.preparation.as_mut().expect("preparation initialized");
        if preparation.transaction_id != transaction_id { return Err(SessionError::State("application preparation changed before escalation".into())); }
        let pending = preparation.pending_escalations().checked_add(1)
            .ok_or_else(|| SessionError::State("pending application escalations overflow".into()))?;
        preparation.escalated = preparation.confirmed_escalation();
        preparation.escalation_pending = Some(pending);
        preparation.outcome = Outcome::Unknown;
        runtime.save_app(&app)
    }).await
}

pub(super) async fn escalation_finished(runtime: &Runtime, prepared: &AppRecord, transaction_id: Option<&str>, sent: bool, deadline: &Deadline) -> Result<(), SessionError> {
    let runtime = runtime.clone();
    let prepared = prepared.clone();
    let transaction_id = transaction_id.map(str::to_string);
    let deadline = deadline.clone();

    blocking(move || {
        let _lease = runtime.app_lease_until(&prepared.unit, &deadline)?;
        let (mut app, archived) = result_record(&runtime, &prepared.unit)?;
        if app.invocation_id != prepared.invocation_id { return Err(SessionError::State("application invocation changed after escalation".into())); }
        let preparation = app.preparation.as_mut().ok_or_else(|| SessionError::State("application preparation is missing after escalation".into()))?;
        if preparation.transaction_id != transaction_id { return Err(SessionError::State("application preparation changed after escalation".into())); }

        let pending = preparation.escalation_pending.and_then(|pending| pending.checked_sub(1))
            .ok_or_else(|| SessionError::State("application has no pending escalation to confirm".into()))?;
        preparation.escalated |= sent;
        preparation.escalation_pending = Some(pending);

        if archived { runtime.archive_app(&app) } else { runtime.save_app(&app) }
    }).await
}

pub(super) async fn finish(runtime: &Runtime, prepared: &AppRecord, transaction_id: Option<&str>, outcome: Outcome, deadline: &Deadline) -> Result<Outcome, SessionError> {
    let runtime = runtime.clone();
    let prepared = prepared.clone();
    let transaction_id = transaction_id.map(str::to_string);
    let deadline = deadline.clone();
    blocking(move || {
        let _lease = runtime.app_lease_until(&prepared.unit, &deadline)?;
        let (mut app, archived) = result_record(&runtime, &prepared.unit)?;
        if app.invocation_id != prepared.invocation_id { return Ok(Outcome::Unknown); }
        let legacy = app.quit_started && app.dispatch.is_none();
        let Some(preparation) = &mut app.preparation else { return Ok(Outcome::Unknown); };
        if preparation.transaction_id != transaction_id { return Ok(Outcome::Unknown); }
        let outcome = match outcome {
            Outcome::Clean if preparation.confirmed_escalation() => Outcome::Forced,
            Outcome::Clean if preparation.uncertain_escalation() => Outcome::Unknown,
            Outcome::Clean if app.invocation_id.is_empty() || legacy => Outcome::Unknown,
            other => other,
        };
        preparation.outcome = outcome;
        if archived { runtime.archive_app(&app)?; } else { runtime.save_app(&app)?; }
        Ok(outcome)
    }).await
}

fn result_record(runtime: &Runtime, unit: &str) -> Result<(AppRecord, bool), SessionError> {
    match runtime.app(unit) {
        Ok(app) => Ok((app, false)),
        Err(SessionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok((runtime.archived_app(unit)?, true)),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "app_result_tests.rs"]
mod tests;

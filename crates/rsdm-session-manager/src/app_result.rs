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
}

pub(super) fn start(app: &mut AppRecord, transaction_id: Option<&str>) {
    let escalated = app.preparation.as_ref().is_some_and(|preparation| preparation.escalated);
    app.preparation = Some(Preparation { transaction_id: transaction_id.map(str::to_string), outcome: Outcome::Unknown, escalated });
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
        preparation.escalated = true;
        preparation.outcome = Outcome::Unknown;
        runtime.save_app(&app)
    }).await
}

pub(super) async fn finish(runtime: &Runtime, prepared: &AppRecord, transaction_id: Option<&str>, outcome: Outcome, deadline: &Deadline) -> Result<Outcome, SessionError> {
    let runtime = runtime.clone();
    let prepared = prepared.clone();
    let transaction_id = transaction_id.map(str::to_string);
    let deadline = deadline.clone();
    blocking(move || {
        let _lease = runtime.app_lease_until(&prepared.unit, &deadline)?;
        let (mut app, archived) = match runtime.app(&prepared.unit) {
            Ok(app) => (app, false),
            Err(SessionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => (runtime.archived_app(&prepared.unit)?, true),
            Err(error) => return Err(error),
        };
        if app.invocation_id != prepared.invocation_id { return Ok(Outcome::Unknown); }
        let legacy = app.quit_started && app.dispatch.is_none();
        let Some(preparation) = &mut app.preparation else { return Ok(Outcome::Unknown); };
        if preparation.transaction_id != transaction_id { return Ok(Outcome::Unknown); }
        let outcome = match outcome {
            Outcome::Clean if preparation.escalated => Outcome::Forced,
            Outcome::Clean if app.invocation_id.is_empty() || legacy => Outcome::Unknown,
            other => other,
        };
        preparation.outcome = outcome;
        if archived { runtime.archive_app(&app)?; } else { runtime.save_app(&app)?; }
        Ok(outcome)
    }).await
}

#[cfg(test)]
#[path = "app_result_tests.rs"]
mod tests;

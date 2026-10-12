//! Durable graceful dispatch claims tied to a transaction and invocation.

use serde::{Deserialize, Serialize};

use super::{SessionError, deadline::Deadline, runtime::{AppRecord, Runtime, blocking}};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Method { Command, Xsmp, WindowClose, Term }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum State { Intent, Sent, NotSent, Cancelled }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Dispatch {
    pub(super) transaction_id: Option<String>,
    pub(super) invocation_id: Vec<u8>,
    pub(super) method: Method,
    pub(super) state: State,
    #[serde(default)]
    pub(super) xsmp_die_state: Option<State>,
}

pub(super) async fn claim(runtime: &Runtime, prepared: &AppRecord, transaction_id: Option<&str>, method: Method, deadline: &Deadline) -> Result<bool, SessionError> {
    let runtime = runtime.clone();
    let prepared = prepared.clone();
    let transaction_id = transaction_id.map(str::to_string);
    let deadline = deadline.clone();
    blocking(move || {
        let _lease = runtime.app_lease_until(&prepared.unit, &deadline)?;
        let mut app = runtime.app(&prepared.unit)?;
        if app.invocation_id != prepared.invocation_id || app.invocation_id.is_empty() {
            return Err(SessionError::State("application invocation changed before dispatch".into()));
        }
        if !eligible(&app, transaction_id.as_deref(), method) { return Ok(false); }
        if app.deadline_usec.is_none() {
            let limit = super::processes::monotonic_usec()?.checked_add(app.policy.timeout_secs * 1_000_000)
                .ok_or_else(|| SessionError::State("application dispatch deadline overflow".into()))?;
            app.deadline_usec = Some(if deadline.get() == 0 { limit } else { limit.min(deadline.get()) });
        }
        app.quit_started = true;
        if method == Method::WindowClose { app.window_close_sent = true; }
        super::app_result::start(&mut app, transaction_id.as_deref());
        app.dispatch = Some(Dispatch { transaction_id, invocation_id: app.invocation_id.clone(), method, state: State::Intent,
            xsmp_die_state: (method == Method::Xsmp).then_some(State::NotSent) });
        runtime.save_app(&app)?;
        Ok(true)
    }).await
}

fn eligible(app: &AppRecord, transaction_id: Option<&str>, method: Method) -> bool {
    if app.dispatch.as_ref().is_some_and(|dispatch| matches!(dispatch.xsmp_die_state, Some(State::Intent | State::Sent))) { return false; }
    if method == Method::WindowClose && app.window_close_sent { return false; }
    if !app.quit_started { return true; }
    let Some(dispatch) = &app.dispatch else { return false; };
    if dispatch.invocation_id != app.invocation_id { return false; }
    if dispatch.state == State::NotSent { return true; }
    method == Method::Xsmp && dispatch.method == Method::Xsmp && dispatch.state == State::Cancelled
        && dispatch.xsmp_die_state == Some(State::NotSent)
        && transaction_id.is_some() && transaction_id != dispatch.transaction_id.as_deref()
}

#[cfg(any(feature = "xsmp", test))]
pub(super) async fn claim_xsmp_die(runtime: &Runtime, prepared: &AppRecord, transaction_id: Option<&str>, deadline: &Deadline) -> Result<Option<super::runtime::SessionRecord>, SessionError> {
    let runtime = runtime.clone();
    let prepared = prepared.clone();
    let transaction_id = transaction_id.map(str::to_string);
    let deadline = deadline.clone();
    blocking(move || {
        let _lease = runtime.app_lease_until(&prepared.unit, &deadline)?;
        let session = runtime.session()?;
        if session.disabled || session.phase == rsdm_core::domain::SessionPhase::Closed
            || session.pending_transaction_id() != transaction_id.as_deref()
        { return Ok(None); }

        let mut app = runtime.app(&prepared.unit)?;
        if app.invocation_id != prepared.invocation_id || app.invocation_id.is_empty() { return Ok(None); }
        let Some(dispatch) = &mut app.dispatch else { return Ok(None); };
        if dispatch.method != Method::Xsmp || dispatch.state != State::Sent
            || dispatch.invocation_id != prepared.invocation_id || dispatch.transaction_id != transaction_id
            || dispatch.xsmp_die_state != Some(State::NotSent)
        { return Ok(None); }

        dispatch.xsmp_die_state = Some(State::Intent);
        runtime.save_app(&app)?;
        Ok(Some(session))
    }).await
}

#[cfg(any(feature = "xsmp", test))]
pub(super) async fn finish_xsmp_die(runtime: &Runtime, prepared: &AppRecord, transaction_id: Option<&str>, state: State, deadline: &Deadline) -> Result<(), SessionError> {
    let runtime = runtime.clone();
    let prepared = prepared.clone();
    let transaction_id = transaction_id.map(str::to_string);
    let deadline = deadline.clone();
    blocking(move || {
        let _lease = runtime.app_lease_until(&prepared.unit, &deadline)?;
        let mut app = runtime.app(&prepared.unit)?;
        if app.invocation_id != prepared.invocation_id { return Err(SessionError::State("application invocation changed after XSMP Die".into())); }
        if let Some(dispatch) = &mut app.dispatch
            && dispatch.method == Method::Xsmp && dispatch.invocation_id == prepared.invocation_id
            && dispatch.transaction_id == transaction_id && dispatch.xsmp_die_state == Some(State::Intent)
        {
            dispatch.xsmp_die_state = Some(state);
            runtime.save_app(&app)?;
        }
        Ok(())
    }).await
}

pub(super) async fn finish(runtime: &Runtime, prepared: &AppRecord, transaction_id: Option<&str>, method: Method, state: State, deadline: &Deadline) -> Result<(), SessionError> {
    let runtime = runtime.clone();
    let prepared = prepared.clone();
    let deadline = deadline.clone();
    let transaction_id = transaction_id.map(str::to_string);
    blocking(move || {
        let _lease = runtime.app_lease_until(&prepared.unit, &deadline)?;
        let mut app = runtime.app(&prepared.unit)?;
        if app.invocation_id != prepared.invocation_id { return Err(SessionError::State("application invocation changed after dispatch".into())); }
        if let Some(dispatch) = &mut app.dispatch
            && dispatch.method == method && dispatch.invocation_id == prepared.invocation_id && dispatch.transaction_id == transaction_id
        {
            dispatch.state = state;
            if method == Method::WindowClose && state == State::NotSent { app.window_close_sent = false; }
            runtime.save_app(&app)?;
        }
        Ok(())
    }).await
}

pub(super) fn cancel(app: &mut AppRecord) {
    if let Some(dispatch) = &mut app.dispatch {
        if dispatch.method == Method::Xsmp && dispatch.state == State::Sent
            && dispatch.xsmp_die_state == Some(State::NotSent)
        { dispatch.state = State::Cancelled; }
        if dispatch.state == State::NotSent { app.quit_started = false; }
    }
}

#[cfg(test)]
#[path = "app_dispatch_tests.rs"]
mod tests;

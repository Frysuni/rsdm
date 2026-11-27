//! Detailed session diagnostics.

use serde::{Deserialize, Serialize};
use zbus::Type;

use crate::{app_dispatch::{Method, State}, app_result::Outcome, control::SessionStatus, runtime::AppRecord};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Capability {
    pub name: String,
    pub state: String,
    pub reason: String,
}

impl Capability {
    pub(super) fn new(name: &str, state: &str, reason: &str) -> Self {
        Self { name: name.into(), state: state.into(), reason: reason.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PreparationStatus {
    pub unit: String,
    pub invocation_id: Vec<u8>,
    pub transaction_id: String,
    pub method: String,
    pub dispatch_state: String,
    pub result: String,
    pub escalated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SessionStatusDetails {
    pub enabled: bool,
    pub state: String,
    pub status: SessionStatus,
    pub capabilities: Vec<Capability>,
    pub limitations: Vec<String>,
    pub preparations: Vec<PreparationStatus>,
}

impl SessionStatusDetails {
    pub(super) fn disabled() -> Self {
        Self { enabled: false, state: "disabled".into(), status: SessionStatus::disabled(),
            capabilities: Vec::new(), limitations: Vec::new(), preparations: Vec::new() }
    }

    pub(super) fn inactive() -> Self {
        Self { enabled: true, state: "inactive".into(), status: SessionStatus {
            generation: String::new(), login_session_id: String::new(), desktop_entry_id: String::new(),
            provider: "none".into(), phase: "inactive".into(), xsmp_available: false, apps: Vec::new(),
        }, capabilities: Vec::new(), limitations: vec!["No session manager coordinator is running.".into()], preparations: Vec::new() }
    }
}

pub(super) fn preparation(app: &AppRecord) -> Option<PreparationStatus> {
    let prepared = app.preparation.as_ref()?;
    let escalated = prepared.escalated;
    let outcome = match prepared.outcome {
        Outcome::Clean | Outcome::Forced if !escalated && prepared.uncertain_escalation() => Outcome::Unknown,
        outcome => outcome,
    };
    let dispatch = app.dispatch.as_ref().filter(|dispatch| dispatch.invocation_id == app.invocation_id);
    Some(PreparationStatus {
        unit: app.unit.clone(), invocation_id: app.invocation_id.clone(),
        transaction_id: prepared.transaction_id.clone().unwrap_or_default(),
        method: dispatch.map_or("unknown", |dispatch| method(dispatch.method)).into(),
        dispatch_state: dispatch.map_or("unknown", |dispatch| {
            let state = match dispatch.xsmp_die_state {
                State::Intent | State::Sent => dispatch.xsmp_die_state,
                _ => dispatch.state,
            };
            match state {
                State::Intent => "intent", State::Sent => "sent", State::NotSent => "not_sent", State::Cancelled => "cancelled",
            }
        }).into(),
        result: match outcome { Outcome::Clean => "clean", Outcome::Forced => "forced", Outcome::Cancelled => "cancelled", Outcome::Unknown => "unknown" }.into(),
        escalated,
    })
}

pub(super) fn method(method: Method) -> &'static str {
    match method { Method::Command => "command", Method::Xsmp => "xsmp", Method::WindowClose => "window-close", Method::Term => "term" }
}

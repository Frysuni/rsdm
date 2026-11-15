//! Observed work and elapsed waits for the terminal power monitor.

use std::{collections::BTreeMap, time::Instant};

use rsdm_session_manager::{PreparationStatus, SessionStatusDetails};
use rsdm_system::shutdown::{Job, LogindStatus};

pub(super) struct App {
    pub method: String,
    pub methods_used: Vec<String>,
    pub timeout: u64,
    pub state: String,
    pub waiting: Option<Instant>,
    pub ended: Option<Instant>,
}

pub(super) struct TrackedJob {
    pub job: Job,
    pub since: Instant,
    pub queued: bool,
    pub ended: Option<Instant>,
}

pub(super) struct Progress {
    pub started: Instant,
    pub phase: String,
    pub provider: String,
    pub apps: BTreeMap<String, App>,
    pub jobs: BTreeMap<u32, TrackedJob>,
    pub logind: Option<LogindStatus>,
    pub session_error: Option<String>,
    pub system_errors: Vec<String>,
    pub outcome: Option<String>,
    pub observed_shutdown: bool,
    baseline: Vec<PreparationStatus>,
}

impl Progress {
    pub fn new() -> Self {
        Self { started: Instant::now(), phase: "Reading shutdown status".into(), provider: String::new(),
            apps: BTreeMap::new(), jobs: BTreeMap::new(), logind: None, session_error: None,
            system_errors: Vec::new(), outcome: None, observed_shutdown: false, baseline: Vec::new() }
    }

    pub fn initialize(&mut self, details: SessionStatusDetails) {
        self.baseline = details.preparations.clone();
        self.session(details);
    }

    pub fn session(&mut self, details: SessionStatusDetails) {
        self.phase = details.status.phase;
        self.provider = details.status.provider;

        for (unit, app) in &mut self.apps {
            if !details.status.apps.iter().any(|(active, ..)| active == unit) && !app.finished() {
                app.state = "no longer tracked".into();
                app.ended.get_or_insert_with(Instant::now);
            }
        }

        for (unit, method, timeout, state) in details.status.apps {
            let app = self.apps.entry(unit).or_insert(App { method: method.clone(), methods_used: Vec::new(), timeout, state: state.clone(), waiting: None, ended: None });
            app.method = method;
            app.state = state;
            if app.state == "quitting" { app.waiting.get_or_insert_with(Instant::now); }
        }

        for prepared in details.preparations {
            let old = self.baseline.iter().any(|old| old.unit == prepared.unit && old.invocation_id == prepared.invocation_id
                && old.transaction_id == prepared.transaction_id && old.result == prepared.result
                && old.dispatch_state == prepared.dispatch_state && old.escalated == prepared.escalated);
            if old { continue; }
            if let Some(app) = self.apps.get_mut(&prepared.unit) {
                app.method = prepared.method;
                if prepared.escalated { app.method.push_str(" + forced termination"); }
                if app.method != "unknown" && !app.methods_used.contains(&app.method) { app.methods_used.push(app.method.clone()); }
                if prepared.result != "unknown" { app.state = prepared.result; }
                if app.finished() || app.state == "cancelled" { app.ended.get_or_insert_with(Instant::now); }
            }
        }

        self.session_error = None;
    }

    pub fn system(&mut self, jobs: Result<Vec<Job>, String>, logind: Result<LogindStatus, String>) {
        self.system_errors.clear();
        match logind {
            Ok(status) => {
                self.observed_shutdown |= status.preparing;
                self.logind = Some(status);
            }
            Err(error) => {
                self.logind = None;
                self.system_errors.push(format!("logind status unavailable: {error}"));
            }
        }

        match jobs {
            Ok(jobs) => {
                self.observed_shutdown |= jobs.iter().any(|job|
                    matches!(job.unit.as_str(), "reboot.target" | "poweroff.target" | "shutdown.target"));
                if !self.observed_shutdown { return; }
                for tracked in self.jobs.values_mut() {
                    tracked.queued = false;
                    tracked.ended.get_or_insert_with(Instant::now);
                }
                for job in jobs {
                    let tracked = self.jobs.entry(job.id).or_insert_with(|| TrackedJob { job: job.clone(), since: Instant::now(), queued: true, ended: None });
                    tracked.job = job;
                    tracked.queued = true;
                    tracked.ended = None;
                }
            }
            Err(error) => self.system_errors.push(format!("systemd status unavailable: {error}")),
        }
    }

    pub fn stage(&self) -> usize {
        if self.observed_shutdown { 2 }
        else if self.outcome.is_some() || self.phase == "stopping_session" || self.apps.is_empty() { 1 }
        else { 0 }
    }
}

impl App {
    pub fn finished(&self) -> bool { matches!(self.state.as_str(), "clean" | "forced") }
}

#[cfg(test)]
#[path = "power_state_tests.rs"]
mod tests;

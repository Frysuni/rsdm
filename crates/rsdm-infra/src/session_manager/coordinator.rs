//! Serial lifecycle decisions; blocking start/stop jobs run in workers.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc::{self, Receiver, Sender}},
    time::{Duration, Instant},
};

use rsdm_core::domain::{SessionManagerConfig, SessionPhase};
use crate::power::{ShutdownMonitor, ShutdownNotice};

use super::{
    SessionError, app_stop::ShutdownControl, bus::UserManager,
    control::{LaunchRequest, Reply, Request, StopOutcome},
    lifecycle::Lifecycle,
    provider::{Provider, StartOptions}, runtime::{Runtime, SessionRecord},
    session_process::SessionProcess,
};

pub(super) enum Work {
    Boot(Result<Vec<u8>, SessionError>),
    Ready(Result<bool, SessionError>),
    Launched(String, Reply<String>, Result<String, SessionError>),
    Prepared(Result<Vec<String>, SessionError>),
    Delegated(Result<(), SessionError>),
    PowerRequested(Result<(), SessionError>),
    Finished(Result<(), SessionError>),
}

pub(super) struct Coordinator {
    pub manager: UserManager,
    pub runtime: Runtime,
    pub record: SessionRecord,
    pub provider: Provider,
    pub lifecycle: Lifecycle,
    pub process: Option<SessionProcess>,
    pub environment: Vec<(String, String)>,
    pub directory: PathBuf,
    pub requests: Receiver<Request>,
    pub work: Receiver<Work>,
    pub events: Sender<Work>,
    pub queued: VecDeque<(LaunchRequest, Reply<String>, Instant)>,
    pub finalize_replies: Vec<Reply<()>>,
    pub stop_replies: Vec<Reply<StopOutcome>>,
    pub stopping: Arc<AtomicBool>,
    pub shutdown: Option<Arc<ShutdownControl>>,
    pub action: String,
    pub ready_busy: bool,
    pub ready_once: bool,
    pub pending_ready: bool,
    pub preparing: bool,
    pub forced_units: Vec<String>,
    pub exit_code: i32,
    pub replies_pending: usize,
    pub workers: usize,
    pub xsmp: super::xsmp::Handle,
    pub control_bus: super::control::ControlServer,
    pub ready_deadline: Instant,
    pub display_since: Option<Instant>,
    pub _power_monitor: ShutdownMonitor,
    pub notices: Receiver<ShutdownNotice>,
    pub last_reap: Instant,
    pub _lease: super::session_lease::SessionLease,
}

pub(super) fn start(argv: &[String], cfg: &SessionManagerConfig, options: &StartOptions) -> Result<i32, SessionError> {
    let mut coordinator = Coordinator::create(argv, cfg, options)?;
    super::signals::install();
    let result = coordinator.run();
    if result.as_ref().map_or(true, |code| *code != 0) {
        coordinator.recover();
    }
    result
}

impl Coordinator {
    fn run(&mut self) -> Result<i32, SessionError> {
        loop {
            self.notifications()?;
            self.control_bus.maintain();
            while let Ok(work) = self.work.try_recv() {
                self.completed(work)?;
            }
            match self.requests.recv_timeout(Duration::from_millis(100)) {
                Ok(request) => self.request(request)?,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(SessionError::State("session control channel closed".into()));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if self.lifecycle.phase == SessionPhase::Closed {
                break;
            }
            self.verify_readiness()?;
            match self.observe() {
                Err(SessionError::Bus(error)) if super::bus::retryable_error(&error) => {
                    tracing::warn!(%error, "deferring session observation while the user manager is unavailable");
                }
                other => other?,
            }
            self.advance()?;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.replies_pending > 0 && Instant::now() < deadline {
            if let Ok(request) = self.requests.recv_timeout(Duration::from_millis(50)) {
                self.request(request)?;
            }
        }
        Ok(self.exit_code)
    }

    pub fn booting(&self) -> bool {
        self.process.as_ref().is_some_and(|process| process.booting)
    }

    pub fn save(&mut self) -> Result<(), SessionError> {
        self.record.phase = self.lifecycle.phase;
        self.runtime.save_session(&self.record)
    }

    fn completed(&mut self, work: Work) -> Result<(), SessionError> {
        self.workers = self.workers.saturating_sub(1);
        match work {
            Work::Boot(result) => self.boot_completed(result)?,
            Work::Ready(result) => self.ready_completed(result)?,
            Work::Launched(unit, reply, result) => {
                self.lifecycle.complete_launch(&unit);
                let _ = reply.try_send(result.map_err(|error| error.to_string()));
            }
            Work::Prepared(result) => self.prepared(result)?,
            Work::Delegated(result) => self.delegated(result)?,
            Work::PowerRequested(result) => self.power_requested(result)?,
            Work::Finished(result) => {
                match result {
                    Ok(()) => self.respond_stop("completed", "session processes stopped"),
                    Err(error) => {
                        tracing::error!(%error, "session cleanup failed");
                        self.exit_code = 1;
                        self.respond_stop("failed", &error.to_string());
                    }
                }
                self.lifecycle.phase = SessionPhase::Closed;
            }
        }
        self.save()
    }

}

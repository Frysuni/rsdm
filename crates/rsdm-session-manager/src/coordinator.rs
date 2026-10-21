//! Serial lifecycle decisions; start/stop jobs run in asynchronous workers.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use rsdm_core::domain::{SessionManagerConfig, SessionPhase};
use rsdm_system::logind::{ShutdownMonitor, ShutdownNotice};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver, UnboundedSender};

use super::{
    SessionError, app_stop::ShutdownControl, bus::UserManager,
    control::{LaunchRequest, Reply, Request, StopOutcome},
    lifecycle::Lifecycle,
    provider::{Provider, StartOptions}, runtime::{Runtime, SessionRecord},
    session_process::SessionProcess,
};

pub(super) const MAX_COORDINATOR_WORKERS: usize = 8;

pub(super) enum Work {
    Observed(super::coordinator_probe::ObservationResult),
    Boot(Result<Vec<u8>, SessionError>),
    TargetsChecked(Result<bool, SessionError>),
    Ready(Result<Option<bool>, SessionError>),
    EnvironmentPublished(Result<(), SessionError>),
    Launched(String, Reply<String>, Result<String, SessionError>),
    Prepared(Result<Vec<String>, SessionError>, Option<super::status::Capability>),
    Delegated(Result<(), SessionError>),
    AttachedReady(Result<Option<super::native_attached::Scoped>, SessionError>, Option<super::status::Capability>),
    PowerRequested(Result<(), SessionError>, Option<super::status::Capability>),
    StatusDetails(Reply<super::status::SessionStatusDetails>, Result<super::status::SessionStatusDetails, SessionError>),
    Finished(SessionProcess, Result<(), SessionError>),
    LifecycleChecked(SessionProcess, super::status::Capability),
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
    pub work: UnboundedReceiver<Work>,
    pub events: UnboundedSender<Work>,
    pub queued: VecDeque<(LaunchRequest, Reply<String>, Instant)>,
    pub finalize_replies: Vec<Reply<()>>,
    pub pending_environment: VecDeque<(Vec<(String, String)>, Reply<()>)>,
    pub environment_reply: Option<(Vec<String>, Reply<()>)>,
    pub stop_replies: Vec<Reply<StopOutcome>>,
    pub stopping: Arc<AtomicBool>,
    pub shutdown: Option<Arc<ShutdownControl>>,
    pub action: String,
    pub ready_busy: bool,
    pub ready_once: bool,
    pub pending_ready: bool,
    pub pending_boot: bool,
    pub preparing: bool,
    pub retain_launcher: bool,
    pub forced_units: Vec<String>,
    pub exit_code: i32,
    pub replies_pending: usize,
    pub workers: usize,
    pub observation_busy: bool,
    pub xsmp: super::xsmp::Handle,
    pub control_bus: super::control::ControlServer,
    pub ready_deadline: Instant,
    pub display_since: Option<Instant>,
    pub _power_monitor: Option<ShutdownMonitor>,
    pub notices: tokio::sync::mpsc::UnboundedReceiver<ShutdownNotice>,
    pub last_reap: Instant,
    pub _lease: super::session_lease::SessionLease,
}

pub(super) async fn start(
    argv: &[String], cfg: &SessionManagerConfig, options: &StartOptions,
    actions: super::action_gate::ActionGate, config_path: Option<PathBuf>,
) -> Result<i32, SessionError> {
    let mut signals = super::signals::Signals::new()?;
    let created = Coordinator::create(argv, cfg, options, actions.clone(), config_path).await;
    let mut coordinator = match created {
        Ok(coordinator) => coordinator,
        Err(_) if actions.check().is_err() => {
            drop(actions);
            return super::plain_launch::run(argv).await;
        },
        Err(error) => return Err(error),
    };
    let launcher = coordinator.process.as_ref().expect("session process").clone();
    let result = coordinator.run(&mut signals).await;
    if actions.check().is_ok() && !coordinator.retain_launcher
        && result.as_ref().map_or(true, |code| *code != 0)
    {
        coordinator.recover().await;
    }
    // Recovery can be interrupted by a validated disable update too. Recheck
    // it before returning to the PAM owner with a still-running compositor.
    if actions.check().is_err() {
        let disabled = coordinator.disable().await;
        let process = coordinator.process.take().unwrap_or(launcher);
        let record = coordinator.record.clone();
        let provider = coordinator.provider.clone();
        let connection = coordinator.manager.connection();
        drop(coordinator);
        drop(actions);
        if process.child.is_none() && !process.submitted.load(std::sync::atomic::Ordering::Acquire) {
            drop(connection);
            disabled?;
            return super::plain_launch::run(argv).await;
        }
        let code = super::session_wait::wait(process, &record, &provider, connection).await?;
        disabled?;
        return Ok(code);
    }
    if coordinator.retain_launcher {
        drop(actions);
        return super::coordinator_lifecycle::wait_retained(coordinator, launcher, result).await;
    }
    if let Err(error) = coordinator.xsmp.shutdown(&coordinator.manager.deadline).await {
        if result.is_ok() { return Err(error); }
        tracing::warn!(%error, "XSMP worker shutdown failed during session recovery");
    }
    result
}

impl Coordinator {
    pub(super) async fn run(&mut self, signals: &mut super::signals::Signals) -> Result<i32, SessionError> {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut notices_open = true;
        loop {
            tokio::select! {
                biased;
                _ = self.manager.actions.wait_closed() => return Ok(self.exit_code),
                request = self.requests.recv() => {
                    let request = request.ok_or_else(|| SessionError::State("session control channel closed".into()))?;
                    self.request(request).await?;
                }
                work = self.work.recv() => {
                    let work = work.ok_or_else(|| SessionError::State("session work channel closed".into()))?;
                    self.completed(work).await?;
                }
                notice = self.notices.recv(), if notices_open => match notice {
                    Some(notice) => self.shutdown_notice(notice).await?,
                    None => notices_open = false,
                },
                received = signals.recv() => {
                    received?;
                    if self.lifecycle.accepts_launch() {
                        let action = if self.provider.kind == super::provider::ProviderKind::Attached { "coordinator-stop" } else { "logout" };
                        self.begin_stop(action, None).await?;
                    }
                }
                _ = tick.tick() => {
                    self.notifications().await?;
                    self.control_bus.maintain();
                    self.start_observation();
                }
            }
            if self.lifecycle.phase == SessionPhase::Closed {
                break;
            }
            self.publish_next_environment().await?;
            self.advance()?;
        }
        self.finish_reply_delivery().await?;
        Ok(self.exit_code)
    }

    pub(super) async fn finish_reply_delivery(&mut self) -> Result<(), SessionError> {
        let limit = Instant::now() + Duration::from_secs(2);
        while self.replies_pending > 0 {
            let local = limit.saturating_duration_since(Instant::now());
            if local.is_zero() { break; }
            let Ok(wait) = self.manager.deadline.remaining(local.min(Duration::from_millis(50))) else { break; };
            match tokio::time::timeout(wait, self.requests.recv()).await {
                Ok(Some(request)) => self.request(request).await?,
                Ok(None) => break,
                Err(_) => {}
            }
        }
        Ok(())
    }

    pub fn booting(&self) -> bool {
        self.observation_busy || self.process.as_ref().is_some_and(|process| process.booting)
    }

    pub async fn save(&mut self) -> Result<(), SessionError> {
        self.record.phase = self.lifecycle.phase;
        let runtime = self.runtime.clone();
        let record = self.record.clone();
        super::runtime::blocking(move || runtime.save_session(&record)).await
    }

    pub(super) async fn completed(&mut self, work: Work) -> Result<(), SessionError> {
        if self.manager.actions.check().is_err() {
            self.disabled_completed(work);
            return Ok(());
        }
        self.workers = self.workers.saturating_sub(1);
        match work {
            Work::StatusDetails(reply, result) => {
                let _ = reply.try_send(result.map_err(|error| error.to_string()));
                return Ok(());
            }
            Work::Observed(result) => self.observation_completed(result).await?,
            Work::Boot(result) => self.boot_completed(result).await?,
            Work::TargetsChecked(result) => self.targets_checked(result).await?,
            Work::Ready(result) => self.ready_completed(result).await?,
            Work::EnvironmentPublished(result) => self.environment_published(result).await?,
            Work::Launched(unit, reply, result) => {
                self.lifecycle.complete_launch(&unit);
                let _ = reply.try_send(result.map_err(|error| error.to_string()));
            }
            Work::Prepared(result, native) => self.prepared(result, native).await?,
            Work::Delegated(result) => self.delegated(result).await?,
            Work::AttachedReady(result, native) => self.attached_ready(result, native).await?,
            Work::PowerRequested(result, native) => self.power_requested(result, native).await?,
            Work::LifecycleChecked(process, capability) => self.lifecycle_checked(process, capability).await?,
            Work::Finished(process, result) => {
                self.process = Some(process);
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
        self.save().await
    }

}

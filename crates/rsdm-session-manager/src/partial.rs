//! Coordinate registered applications without owning a compositor or PAM login.

use std::{collections::VecDeque, sync::{Arc, atomic::AtomicBool}, time::Instant};

use rsdm_core::domain::{SessionManagerConfig, SessionPhase};
use rsdm_system::logind::ShutdownMonitor;

use super::{SessionError, action_gate::ActionGate, attach_bootstrap::Snapshot, bus::UserManager, control,
    coordinator::Coordinator, lifecycle::Lifecycle, provider::{Provider, ProviderKind}, runtime::{Runtime, SessionRecord}};

pub(super) async fn serve(cfg: &SessionManagerConfig, actions: ActionGate, path: &std::path::Path) -> Result<i32, SessionError> {
    if !cfg.enabled { return Ok(0); }
    let snapshot = actions.guard(super::attach_bootstrap::receive()).await??;
    let mut signals = super::signals::Signals::new()?;
    let mut coordinator = create(actions.clone(), path, snapshot).await?;
    let result = coordinator.run(&mut signals).await;
    if actions.check().is_err() { coordinator.disable().await?; return Ok(0); }
    if result.is_err() { coordinator.recover().await; }
    coordinator.xsmp.shutdown(&coordinator.manager.deadline).await?;
    result
}

async fn create(actions: ActionGate, path: &std::path::Path, snapshot: Snapshot) -> Result<Coordinator, SessionError> {
    let lease = actions.guard(super::runtime::blocking(super::session_lease::SessionLease::acquire)).await??;
    let manager = UserManager::connect_guarded(super::deadline::Deadline::default(), actions.clone()).await?;
    let invocation = manager.invocation_id(super::attach::UNIT).await?;
    let pid: u32 = manager.unit_property(super::attach::UNIT, "org.freedesktop.systemd1.Service", "MainPID").await?;
    if pid != std::process::id() || invocation.is_empty() || invocation.iter().all(|byte| *byte == 0) {
        return Err(SessionError::State("serve is not the current transient coordinator process".into()));
    }
    let identity = snapshot.identity;
    let (endpoint, requests) = control::Endpoint::for_session(identity.clone());
    let generation = identity.generation.clone();
    let runtime = super::runtime::blocking(move || Runtime::create(&generation)).await?;
    let provider = Provider { kind: ProviderKind::Attached, native_unit: None, logout_command: Vec::new() };
    let xsmp = actions.guard(super::xsmp::Handle::start(&manager, &runtime, &provider)).await??;
    let mut environment = identity.command_environment(snapshot.environment);
    for (name, value) in &xsmp.environment {
        environment.retain(|(existing, _)| existing != name);
        environment.push((name.clone(), value.clone()));
    }
    let path = path.to_path_buf();
    let config_path = super::runtime::blocking(move || Ok(std::path::absolute(path)?)).await?;
    let record = SessionRecord {
        anchor_unit: format!("rsdm-session-{}.service", identity.generation), identity,
        compositor_unit: None, compositor_invocation: Vec::new(), coordinator_unit: Some((super::attach::UNIT.into(), invocation)),
        provider: provider.name().into(), owns_targets: false, phase: SessionPhase::Running, disabled: false,
        config_path: Some(config_path), exported_environment: environment.clone(), shutdown_deadline_usec: None,
    };
    let records = runtime.clone();
    let saved = record.clone();
    actions.guard(super::runtime::blocking(move || records.save_session(&saved))).await??;
    let (notice_sender, notices) = tokio::sync::mpsc::unbounded_channel();
    let power = actions.guard(ShutdownMonitor::start(notice_sender, "Saving registered graphical applications")).await??;
    let bus = actions.guard(control::ControlServer::start(endpoint)).await??;
    let (events, work) = tokio::sync::mpsc::unbounded_channel();
    let mut lifecycle = Lifecycle::new();
    lifecycle.phase = SessionPhase::Running;
    Ok(Coordinator {
        manager, runtime, record, provider, lifecycle, process: None, environment, directory: snapshot.directory,
        requests, work, events, queued: VecDeque::new(), finalize_replies: Vec::new(),
        pending_environment: VecDeque::new(), environment_reply: None, stop_replies: Vec::new(),
        stopping: Arc::new(AtomicBool::new(false)), shutdown: None, action: String::new(), ready_busy: false,
        ready_once: false, pending_ready: false, pending_boot: false, preparing: false, forced_units: Vec::new(),
        exit_code: 0, replies_pending: 0, workers: 0, observation_busy: false, xsmp, control_bus: bus,
        ready_deadline: Instant::now(), display_since: None, _power_monitor: Some(power), notices,
        last_reap: Instant::now(), _lease: lease,
    })
}

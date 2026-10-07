//! Prepare session ownership before spawning the compositor.

use std::{collections::VecDeque, sync::{Arc, atomic::AtomicBool, mpsc}, thread, time::{Duration, Instant}};

use rsdm_core::domain::{SessionManagerConfig, SessionPhase};
use crate::power::ShutdownMonitor;

use super::{
    SessionError, bus::UserManager, control, coordinator::{Coordinator, Work}, env,
    identity::SessionIdentity, lifecycle::Lifecycle, provider::{Provider, ProviderKind, StartOptions},
    runtime::{Runtime, SessionRecord}, session_process::SessionProcess,
    unit_name::{ManagedUnitKind, unique_unit_name}, units,
};

impl Coordinator {
    pub(super) fn create(
        argv: &[String], cfg: &SessionManagerConfig, options: &StartOptions,
    ) -> Result<Self, SessionError> {
        let program = argv.first().ok_or(SessionError::EmptyCommand)?;
        let ready_deadline = Instant::now().checked_add(Duration::from_secs(cfg.ready_timeout_secs))
            .ok_or_else(|| SessionError::State("readiness timeout is too large".into()))?;
        let identity = SessionIdentity::discover()?;
        let (sender, requests) = mpsc::channel();
        // Acquire the one-session lease before changing shared manager state.
        let bus = control::serve(control::Endpoint { requests: sender, uid: identity.uid })?;
        let manager = UserManager::connect()?;
        super::processes::require_pidfds()?;
        if manager.active(units::SESSION_TARGET)? {
            return Err(SessionError::SessionAlreadyActive);
        }
        let provider = Provider::select(argv, &std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(), options)?;
        let (notices_sender, notices) = mpsc::channel();
        let power = ShutdownMonitor::start(notices_sender)
            .map_err(|error| SessionError::State(format!("creating shutdown delay monitor: {error}")))?;
        let runtime = Runtime::create(&identity.generation)?;
        if runtime.path.join("session.toml").try_exists()? {
            return Err(SessionError::State("this session generation already has a recovery record".into()));
        }
        let xsmp = super::xsmp::Handle::start(&manager, &runtime, &provider)?;
        let mut environment = identity.command_environment(env::command_environment());
        for (name, value) in &xsmp.environment {
            environment.retain(|(existing, _)| existing != name);
            environment.push((name.clone(), value.clone()));
        }
        let directory = std::env::current_dir()?;
        let managed_unit = format!("{}.service", unique_unit_name(ManagedUnitKind::Session, program));
        let record = initial_record(identity, &provider, &managed_unit, &xsmp.environment);
        let properties = if provider.kind == ProviderKind::Managed {
            Some(units::compositor_properties(argv, &environment, &directory, &record.anchor_unit)?)
        } else {
            None
        };

        super::startup::publish(&manager, &runtime, &record)?;
        let process = match SessionProcess::launch(argv, &environment, &directory, &provider, managed_unit) {
            Ok(process) => process,
            Err(error) => {
                super::startup::rollback(&manager, &runtime, &record);
                return Err(error);
            }
        };
        let (events, work) = mpsc::channel();
        let workers = usize::from(properties.is_some());
        if let Some(properties) = properties {
            let manager = manager.clone();
            let unit = process.unit.clone().expect("managed compositor unit");
            let events = events.clone();
            thread::spawn(move || {
                let result = manager.start_service(&unit, &properties).and_then(|()| manager.invocation_id(&unit));
                let _ = events.send(Work::Boot(result));
            });
        }
        Ok(Self {
            manager,
            runtime,
            record,
            provider,
            lifecycle: Lifecycle::new(),
            process: Some(process),
            environment,
            directory,
            requests,
            work,
            events,
            queued: VecDeque::new(),
            finalize_replies: Vec::new(),
            stop_replies: Vec::new(),
            stopping: Arc::new(AtomicBool::new(false)),
            shutdown: None,
            action: String::new(),
            ready_busy: false,
            ready_once: false,
            preparing: false,
            forced_units: Vec::new(),
            exit_code: 0,
            replies_pending: 0,
            workers,
            _control_bus: bus,
            xsmp,
            ready_deadline,
            display_since: None,
            _power_monitor: power,
            notices,
            last_reap: Instant::now(),
        })
    }
}

fn initial_record(
    identity: SessionIdentity, provider: &Provider, managed_unit: &str,
    xsmp_environment: &[(String, String)],
) -> SessionRecord {
    let mut base = env::collect_present(&[], &[]);
    base.retain(|(name, _)| !env::WAYLAND_VARS.contains(&name.as_str()));
    let mut exported_environment = identity.command_environment(base);
    exported_environment.extend_from_slice(xsmp_environment);
    let compositor_unit = if provider.kind == ProviderKind::Managed {
        Some(managed_unit.to_string())
    } else {
        provider.native_unit.clone()
    };
    SessionRecord {
        anchor_unit: format!("rsdm-session-{}.service", identity.generation),
        identity,
        compositor_unit,
        compositor_invocation: Vec::new(),
        provider: provider.name().into(),
        owns_targets: false,
        phase: SessionPhase::Starting,
        exported_environment,
        shutdown_deadline_usec: None,
    }
}

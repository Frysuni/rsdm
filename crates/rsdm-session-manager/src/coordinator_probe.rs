//! Query the user manager and compositor state outside the coordinator actor.

use std::time::Instant;

use rsdm_core::domain::SessionPhase;

use super::{
    SessionError, activation, apps,
    provider::{Provider, ProviderKind}, runtime::Runtime,
    session_process::SessionProcess, units,
};

pub(super) struct ObservationResult {
    pub(super) process: Option<SessionProcess>,
    pub(super) result: Result<Observation, SessionError>,
}

pub(super) struct Observation {
    pub(super) reaped_apps: bool,
    pub(super) boot: Option<BootCheck>,
    pub(super) readiness_check: Option<Result<bool, SessionError>>,
    pub(super) process_exit: Option<ProcessExit>,
    pub(super) anchor_active: Option<bool>,
    pub(super) session_target_active: Option<bool>,
    pub(super) readiness: Option<Readiness>,
}

pub(super) enum BootCheck {
    Pending,
    Discard,
    Completed(Result<Vec<u8>, SessionError>),
}

pub(super) struct ProcessExit {
    pub(super) code: i32,
    pub(super) native_running: bool,
    pub(super) native_starting: bool,
}

pub(super) struct Readiness {
    pub(super) target_active: bool,
    pub(super) display_published: bool,
    pub(super) provider_ready: Option<bool>,
}

pub(super) fn observe_external(
    manager: &super::bus::UserManager,
    runtime: &Runtime,
    provider: &Provider,
    process: Option<SessionProcess>,
    reaped_apps: bool,
    phase: SessionPhase,
    ready_once: bool,
    ready_busy: bool,
    ready_deadline: Instant,
    pending_boot: bool,
    pending_ready: bool,
    anchor_unit: &str,
) -> ObservationResult {
    let mut process = process;
    let result = (|| {
        if reaped_apps {
            apps::release_closed(manager, runtime)?;
        }

        let boot = if pending_boot {
            Some(verify_boot_external(manager, runtime, process.as_ref(), phase)?)
        } else {
            None
        };
        let readiness_check = if pending_ready {
            Some(if matches!(phase, SessionPhase::Starting | SessionPhase::Running) {
                Ok(manager.active(anchor_unit)?)
            } else {
                Ok(false)
            })
        } else {
            None
        };
        let process_exit = observe_process(
            manager,
            provider,
            process.as_mut(),
            phase,
            ready_deadline,
        )?;
        let (anchor_active, session_target_active) = if ready_once
            && matches!(phase, SessionPhase::Starting | SessionPhase::Running)
        {
            (
                Some(manager.active(anchor_unit)?),
                Some(manager.active(units::SESSION_TARGET)?),
            )
        } else {
            (None, None)
        };

        let readiness = if phase == SessionPhase::Starting
            && !ready_busy
            && !process.as_ref().is_some_and(|process| process.booting)
            && Instant::now() < ready_deadline
        {
            let target_active = manager.active(units::SESSION_TARGET)?;
            let display_published = activation::published_display(manager)?;
            let provider_ready = if provider.kind != ProviderKind::Managed {
                Some(provider.ready(manager)?)
            } else {
                None
            };
            Some(Readiness {
                target_active,
                display_published,
                provider_ready,
            })
        } else {
            None
        };

        Ok(Observation {
            reaped_apps,
            boot,
            readiness_check,
            process_exit,
            anchor_active,
            session_target_active,
            readiness,
        })
    })();
    ObservationResult { process, result }
}

fn verify_boot_external(
    manager: &super::bus::UserManager,
    runtime: &Runtime,
    process: Option<&SessionProcess>,
    phase: SessionPhase,
) -> Result<BootCheck, SessionError> {
    if !matches!(phase, SessionPhase::Starting | SessionPhase::Running) {
        return Ok(BootCheck::Discard);
    }
    let unit = process
        .and_then(|process| process.unit.as_deref())
        .ok_or_else(|| SessionError::State("pending compositor start has no unit".into()))?;
    let job = manager.unit_property::<(u32, zbus::zvariant::OwnedObjectPath)>(
        unit,
        "org.freedesktop.systemd1.Unit",
        "Job",
    )?;
    if job.0 != 0 {
        return Ok(BootCheck::Pending);
    }
    let result = match super::processes::generation_invocation(manager, unit, &runtime.generation)? {
        Some(id) if id.iter().any(|byte| *byte != 0) => Ok(id),
        Some(_) => Err(SessionError::State("compositor start did not create an invocation".into())),
        None => Err(SessionError::State("compositor start unit disappeared".into())),
    };
    Ok(BootCheck::Completed(result))
}

fn observe_process(
    manager: &super::bus::UserManager,
    provider: &Provider,
    process: Option<&mut SessionProcess>,
    phase: SessionPhase,
    ready_deadline: Instant,
) -> Result<Option<ProcessExit>, SessionError> {
    let Some(process) = process else {
        return Ok(None);
    };
    let code = process.exited(manager)?;
    let Some(code) = code else {
        return Ok(None);
    };
    let detached = provider.native_desktop()
        || (provider.kind == ProviderKind::External && provider.native_unit.is_none());
    let native_running = detached && code == 0 && provider.ready(manager)?;
    let native_starting = detached
        && code == 0
        && phase == SessionPhase::Starting
        && Instant::now() < ready_deadline;
    Ok(Some(ProcessExit {
        code,
        native_running,
        native_starting,
    }))
}

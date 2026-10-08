//! Observe compositor identity and external stops without blocking the actor.

use std::{thread, time::{Duration, Instant}};

use rsdm_core::domain::SessionPhase;

use super::{
    SessionError,
    activation,
    apps,
    coordinator::{Coordinator, Work},
    processes::monotonic_usec,
    provider::{Provider, ProviderKind},
    runtime::Runtime,
    session_process::SessionProcess,
    units,
};

pub(super) struct ObservationResult {
    pub(super) process: Option<SessionProcess>,
    pub(super) result: Result<Observation, SessionError>,
}

pub(super) struct Observation {
    reaped_apps: bool,
    boot: Option<BootCheck>,
    readiness_check: Option<Result<bool, SessionError>>,
    process_exit: Option<ProcessExit>,
    anchor_active: Option<bool>,
    session_target_active: Option<bool>,
    readiness: Option<Readiness>,
}

enum BootCheck {
    Pending,
    Discard,
    Completed(Result<Vec<u8>, SessionError>),
}

struct ProcessExit {
    code: i32,
    native_running: bool,
    native_starting: bool,
}

struct Readiness {
    target_active: bool,
    display_published: bool,
    provider_ready: Option<bool>,
}

impl Coordinator {
    #[cfg(test)]
    pub fn observe(&mut self) -> Result<(), SessionError> {
        let process = self.process.take();
        let result = observe_external(
            &self.manager,
            &self.runtime,
            &self.provider,
            process,
            self.lifecycle.phase == SessionPhase::Running
                && self.last_reap.elapsed() >= Duration::from_secs(1),
            self.lifecycle.phase,
            self.ready_once,
            self.ready_busy,
            self.ready_deadline,
            self.pending_boot,
            self.pending_ready,
            &self.record.anchor_unit,
        );
        self.observation_completed(result)
    }

    pub fn start_observation(&mut self) {
        if self.observation_busy {
            return;
        }
        if super::signals::requested() && self.lifecycle.accepts_launch() {
            if let Err(error) = self.begin_stop("logout", None) {
                tracing::error!(%error, "cannot begin signal-triggered session stop");
            }
        }

        let reaped_apps = self.lifecycle.phase == SessionPhase::Running
            && self.last_reap.elapsed() >= Duration::from_secs(1);
        let process = self.process.take();
        let manager = self.manager.clone();
        let runtime = self.runtime.clone();
        let provider = self.provider.clone();
        let phase = self.lifecycle.phase;
        let ready_once = self.ready_once;
        let ready_busy = self.ready_busy;
        let ready_deadline = self.ready_deadline;
        let pending_boot = self.pending_boot;
        let pending_ready = self.pending_ready;
        let anchor_unit = self.record.anchor_unit.clone();
        let events = self.events.clone();
        self.observation_busy = true;
        self.workers += 1;

        thread::spawn(move || {
            let result = observe_external(
                &manager,
                &runtime,
                &provider,
                process,
                reaped_apps,
                phase,
                ready_once,
                ready_busy,
                ready_deadline,
                pending_boot,
                pending_ready,
                &anchor_unit,
            );
            let _ = events.send(Work::Observed(result));
        });
    }

    pub fn observation_completed(
        &mut self,
        result: ObservationResult,
    ) -> Result<(), SessionError> {
        self.observation_busy = false;
        self.process = result.process;
        let observation = match result.result {
            Ok(observation) => observation,
            Err(SessionError::Bus(error)) if super::bus::retryable_error(&error) => {
                tracing::warn!(%error, "deferring session observation while the user manager is unavailable");
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        if let Some(boot) = observation.boot {
            match boot {
                BootCheck::Pending => {}
                BootCheck::Discard => {
                    self.pending_boot = false;
                    if let Some(process) = &mut self.process {
                        process.booting = false;
                    }
                }
                BootCheck::Completed(result) => self.boot_completed(result)?,
            }
        }
        if let Some(result) = observation.readiness_check {
            self.complete_readiness(result?)?;
        }

        if observation.reaped_apps {
            self.last_reap = Instant::now();
        }
        if let Some(process) = &self.process
            && self.record.compositor_invocation != process.invocation
        {
            self.record.compositor_invocation = process.invocation.clone();
            self.save()?;
        }
        if let Some(exit) = observation.process_exit
            && !exit.native_running
            && !exit.native_starting
        {
            self.exit_code = exit.code;
            if self.lifecycle.phase != SessionPhase::StoppingSession {
                self.begin_stop(
                    "compositor-exited",
                    Some(monotonic_usec()?.saturating_add(5_000_000)),
                )?;
            }
        }
        if self.ready_once
            && self.lifecycle.phase != SessionPhase::StoppingSession
            && let (Some(anchor_active), Some(session_target_active)) = (
                observation.anchor_active,
                observation.session_target_active,
            )
            && (!anchor_active || !session_target_active)
        {
            self.begin_stop(
                "external-stop",
                Some(monotonic_usec()?.saturating_add(5_000_000)),
            )?;
        }
        if let Some(readiness) = observation.readiness {
            if readiness.display_published {
                self.display_since.get_or_insert_with(Instant::now);
            } else {
                self.display_since = None;
            }
            let display_ready = self.display_since.is_some_and(|time| {
                time.elapsed() >= Duration::from_secs(2)
            });
            let native = self.provider.kind != ProviderKind::Managed;
            let ready = if native {
                readiness.target_active && readiness.provider_ready.unwrap_or(false)
            } else {
                readiness.target_active || display_ready
            };
            if ready {
                self.activate();
            }
        }

        while self.queued.front().is_some_and(|(_, _, accepted)| {
            accepted.elapsed() >= Duration::from_secs(10)
        }) {
            let (_, reply, _) = self.queued.pop_front().expect("queued launch");
            let _ = reply.try_send(Err("graphical session did not become ready".into()));
        }
        Ok(())
    }
}

fn observe_external(
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
            && phase != SessionPhase::StoppingSession
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

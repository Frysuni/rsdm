//! Ambiguous job replies are reconciled through reads, never replayed.

use super::*;
use std::{future::poll_fn, pin::Pin, task::Poll};
use zbus::export::futures_core::Stream;

enum Operation {
    Start { new_unit: bool, generation: Option<String> },
    Stop,
}

pub(super) async fn start(
    manager: &UserManager, unit: &str, properties: &UnitProperties, timeout: Duration,
    submitted: &AtomicBool, quit: Option<(&crate::app_stop::ShutdownControl, &crate::runtime::AppRecord)>,
) -> Result<(), SessionError> {
    let deadline = Instant::now().checked_add(timeout)
        .ok_or_else(|| SessionError::State("systemd job deadline overflow".into()))?;
    within_deadline(unit, deadline, async {
        let new_unit = match manager.read_async(|| async {
            manager.proxy().await?.call::<_, _, OwnedObjectPath>("GetUnit", &(unit,)).await
        }, Duration::from_secs(5)).await {
            Ok(_) => false,
            Err(error) if missing_unit(&error) => true,
            Err(error) => return Err(error.into()),
        };
        let generation = generation(properties)?;
        let proxy = manager.request(manager.proxy()).await?;
        let mut signals = manager.request(proxy.receive_signal_with_args("JobRemoved", &[(2, unit)])).await?;
        let auxiliary: Vec<(&str, UnitProperties)> = Vec::new();
        let mut reference = None;
        let reply = manager.request(async {
            if let Some((control, app)) = quit { verify_quit_app(&proxy, app, control).await?; }
            if Instant::now() >= deadline { return Err(job_timeout(unit)); }
            manager.deadline.remaining(Duration::MAX)?;
            if quit.is_some_and(|(control, _)| control.cancelled()) {
                return Err(zbus::fdo::Error::Failed("application shutdown cancelled before service start".into()).into());
            }
            manager.actions.check().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
            reference = if new_unit && properties.iter().any(|(name, value)| *name == "AddRef" && matches!(value, Variant::Bool(true))) {
                generation.as_ref().map(|generation| manager.transport.stage_reference(unit, generation.clone()))
            } else { None };
            if let Some(reference) = &mut reference { reference.submitted(); }
            submitted.store(true, Ordering::Relaxed);
            proxy.call("StartTransientUnit", &(unit, "fail", properties, auxiliary)).await
        }).await;
        if let Err(error) = &reply
            && !submitted.load(Ordering::Relaxed)
        { return Err(error.clone().into()); }
        if let Err(error) = &reply
            && rejected_start(manager, unit, error).await
        {
            let reference_retained = reference.take().is_some_and(|reference| !reference.reject());
            return Err(SessionError::StartRejected { error: Box::new(error.clone().into()), reference_retained });
        }
        complete(manager, unit, &mut signals, reply, Operation::Start { new_unit, generation }, deadline).await
    }).await
}

async fn verify_quit_app(manager: &Proxy<'_>, app: &crate::runtime::AppRecord, control: &crate::app_stop::ShutdownControl) -> zbus::Result<()> {
    if control.cancelled() {
        return Err(zbus::fdo::Error::Failed("application shutdown cancelled before service start".into()).into());
    }
    let path: OwnedObjectPath = manager.call("GetUnit", &(&app.unit,)).await?;
    let unit = zbus::proxy::Builder::<Proxy<'_>>::new(manager.connection()).destination(DESTINATION).path(path)
        .interface("org.freedesktop.systemd1.Unit").cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    let invocation: Vec<u8> = unit.get_property("InvocationID").await?;
    if app.invocation_id.is_empty() || app.invocation_id.iter().all(|byte| *byte == 0) || invocation != app.invocation_id {
        return Err(zbus::fdo::Error::AccessDenied("application invocation changed before quit command dispatch".into()).into());
    }
    Ok(())
}

async fn rejected_start(manager: &UserManager, unit: &str, error: &zbus::Error) -> bool {
    if matches!(error, zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.systemd1.UnitExists") {
        return true;
    }
    let denied = match error {
        zbus::Error::FDO(error) => matches!(**error, zbus::fdo::Error::InvalidArgs(_)
            | zbus::fdo::Error::AccessDenied(_) | zbus::fdo::Error::UnknownProperty(_)
            | zbus::fdo::Error::PropertyReadOnly(_) | zbus::fdo::Error::NotSupported(_)),
        _ => false,
    };
    if !denied { return false; }
    // Even a rejected request can create a partial unit and acquire AddRef.
    // Only an authoritative missing-unit reply proves there is no ownership.
    matches!(manager.read_async(|| async {
        manager.proxy().await?.call::<_, _, OwnedObjectPath>("GetUnit", &(unit,)).await
    }, Duration::from_secs(5)).await, Err(error) if missing_unit(&error))
}

pub(super) async fn stop(manager: &UserManager, unit: &str, timeout: Duration) -> Result<(), SessionError> {
    let deadline = Instant::now().checked_add(timeout)
        .ok_or_else(|| SessionError::State("systemd job deadline overflow".into()))?;
    within_deadline(unit, deadline, async {
        match manager.read_async(|| async {
            manager.proxy().await?.call::<_, _, OwnedObjectPath>("GetUnit", &(unit,)).await
        }, Duration::from_secs(5)).await {
            Ok(_) => {},
            Err(error) if missing_unit(&error) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let proxy = manager.request(manager.proxy()).await?;
        let mut signals = manager.request(proxy.receive_signal_with_args("JobRemoved", &[(2, unit)])).await?;
        let reply = manager.request(async {
            if Instant::now() >= deadline { return Err(job_timeout(unit)); }
            proxy.call::<_, _, OwnedObjectPath>("StopUnit", &(unit, "replace")).await
        }).await;
        if reply.as_ref().is_err_and(missing_unit) { return Ok(()); }
        complete(manager, unit, &mut signals, reply, Operation::Stop, deadline).await
    }).await
}

async fn within_deadline<T>(
    unit: &str, deadline: Instant, operation: impl Future<Output = Result<T, SessionError>>,
) -> Result<T, SessionError> {
    if Instant::now() >= deadline { return Err(job_timeout(unit).into()); }
    tokio::select! {
        biased;
        result = operation => result,
        _ = tokio::time::sleep_until(deadline.into()) => Err(job_timeout(unit).into()),
    }
}

fn job_timeout(unit: &str) -> zbus::Error {
    zbus::fdo::Error::TimedOut(format!("systemd job deadline expired for {unit}")).into()
}

fn generation(properties: &UnitProperties) -> Result<Option<String>, SessionError> {
    let Some((_, value)) = properties.iter().find(|(name, _)| *name == "Environment") else { return Ok(None); };
    let environment: Vec<String> = Vec::try_from(OwnedVariant::try_from(value).map_err(zbus::Error::from)?)
        .map_err(zbus::Error::from)?;
    Ok(environment.into_iter().find_map(|entry| {
        entry.strip_prefix("RSDM_SESSION_GENERATION=").map(str::to_string)
    }))
}

async fn complete(
    manager: &UserManager, unit: &str, signals: &mut zbus::proxy::SignalStream<'_>,
    reply: zbus::Result<OwnedObjectPath>, operation: Operation, deadline: Instant,
) -> Result<(), SessionError> {
    let job = match reply {
        Ok(job) => Some(job),
        Err(error) if retryable_error(&error) => None,
        Err(error) => return Err(error.into()),
    };
    // Reconcile missing replies/signals inside the original operation budget.
    // Never start a second wait after the deadline or replay the mutation.
    loop {
        if Instant::now() >= deadline { return Err(job_timeout(unit).into()); }
        if let Some(job) = &job {
            if drain_job(signals, job).await? { return Ok(()); }
        }
        let remaining = deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(5));
        let complete = manager.read_async(|| inspect(manager, unit, &operation), remaining).await;
        if let Some(job) = &job {
            if drain_job(signals, job).await? { return Ok(()); }
        }
        match complete {
            Ok(true) => return Ok(()),
            Ok(false) => {},
            // A short-lived start can be collected before GetUnit sees it.
            // Its exact JobRemoved signal can still confirm completion.
            Err(error) if job.is_some() && missing_unit(&error) => {},
            Err(error) => return Err(error.into()),
        }
        tokio::time::sleep_until((Instant::now() + Duration::from_millis(50)).min(deadline).into()).await;
    }
}

async fn inspect(manager: &UserManager, unit: &str, operation: &Operation) -> zbus::Result<bool> {
    let path = match manager.proxy().await?.call::<_, _, OwnedObjectPath>("GetUnit", &(unit,)).await {
        Ok(path) => path,
        Err(error) if missing_unit(&error) => return Ok(matches!(operation, Operation::Stop)),
        Err(error) => return Err(error),
    };
    let connection = manager.connection();
    let proxy = zbus::proxy::Builder::<Proxy<'_>>::new(&connection).destination(DESTINATION).path(path.clone())
        .interface("org.freedesktop.systemd1.Unit").cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    let id: String = proxy.get_property("Id").await?;
    if id != unit { return Err(zbus::Error::Failure(format!("{unit} resolved to another unit"))); }
    let (pending, _): (u32, OwnedObjectPath) = proxy.get_property("Job").await?;
    if pending != 0 { return Ok(false); }
    let state: String = proxy.get_property("ActiveState").await?;
    match operation {
        Operation::Stop => {
            if !matches!(state.as_str(), "inactive" | "failed") { return Ok(false); }
            let processes: Vec<(String, u32, String)> = manager.proxy().await?.call("GetUnitProcesses", &(unit,)).await?;
            Ok(processes.is_empty())
        }
        Operation::Start { new_unit, generation } => {
            if !new_unit { return Err(zbus::Error::Failure(format!("{unit} existed before its start request"))); }
            if !proxy.get_property::<bool>("Transient").await? {
                return Err(zbus::Error::Failure(format!("{unit} is not the requested transient unit")));
            }
            inspect_start(manager, path, state, generation).await
        }
    }
}

async fn inspect_start(
    manager: &UserManager, path: OwnedObjectPath, state: String, generation: &Option<String>,
) -> zbus::Result<bool> {
    let connection = manager.connection();
    let proxy = zbus::proxy::Builder::<Proxy<'_>>::new(&connection).destination(DESTINATION).path(path)
        .interface("org.freedesktop.systemd1.Service").cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    if let Some(generation) = generation {
        let environment: Vec<String> = proxy.get_property("Environment").await?;
        if !environment.contains(&format!("RSDM_SESSION_GENERATION={generation}")) {
            return Err(zbus::Error::Failure("started unit belongs to another session generation".into()));
        }
    }
    if matches!(state.as_str(), "active" | "reloading") { return Ok(true); }
    if state == "inactive" {
        let result: String = proxy.get_property("Result").await?;
        let code: i32 = proxy.get_property("ExecMainCode").await?;
        let status: i32 = proxy.get_property("ExecMainStatus").await?;
        if result == "success" && code == libc::CLD_EXITED && status == 0 { return Ok(true); }
    }
    if matches!(state.as_str(), "failed" | "inactive") {
        return Err(zbus::Error::Failure(format!("transient service start ended in {state}")));
    }
    Ok(false)
}

fn job_message(message: zbus::Message, job: &OwnedObjectPath) -> Result<bool, SessionError> {
    let (_, path, unit, result): (u32, OwnedObjectPath, String, String) = message.body().deserialize()?;
    if path != *job { return Ok(false); }
    if result == "done" { return Ok(true); }
    Err(SessionError::State(format!("{unit} job completed with {result}")))
}

async fn drain_job(signals: &mut zbus::proxy::SignalStream<'_>, job: &OwnedObjectPath) -> Result<bool, SessionError> {
    while let Poll::Ready(Some(message)) = poll_fn(|cx| Poll::Ready(Pin::new(&mut *signals).poll_next(cx))).await {
        if job_message(message, job)? { return Ok(true); }
    }
    Ok(false)
}

#[cfg(test)]
#[path = "bus_jobs_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "bus_jobs_fixture.rs"]
mod fixture;

#[cfg(test)]
#[path = "bus_start_tests.rs"]
mod start_tests;

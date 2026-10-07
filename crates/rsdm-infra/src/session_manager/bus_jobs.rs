//! Ambiguous job replies are reconciled through reads, never replayed.

use super::*;
use futures_lite::StreamExt;

enum Operation {
    Start { new_unit: bool, generation: Option<String> },
    Stop,
}

pub(super) async fn start(
    manager: &UserManager, unit: &str, properties: &UnitProperties, timeout: Duration,
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
        if new_unit && properties.iter().any(|(name, value)| *name == "AddRef" && matches!(value, Value::Bool(true))) {
            if let Some(generation) = &generation { manager.transport.track(unit, generation.clone()); }
        }
        let proxy = manager.proxy().await?;
        let mut signals = proxy.receive_signal_with_args("JobRemoved", &[(2, unit)]).await?;
        let auxiliary: Vec<(&str, UnitProperties)> = Vec::new();
        let reply = proxy.call("StartTransientUnit", &(unit, "fail", properties, auxiliary)).await;
        complete(manager, unit, &mut signals, reply, Operation::Start { new_unit, generation }, deadline).await
    }).await
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
        let proxy = manager.proxy().await?;
        let mut signals = proxy.receive_signal_with_args("JobRemoved", &[(2, unit)]).await?;
        let reply = proxy.call::<_, _, OwnedObjectPath>("StopUnit", &(unit, "replace")).await;
        if reply.as_ref().is_err_and(missing_unit) { return Ok(()); }
        complete(manager, unit, &mut signals, reply, Operation::Stop, deadline).await
    }).await
}

async fn within_deadline<T>(
    unit: &str, deadline: Instant, operation: impl Future<Output = Result<T, SessionError>>,
) -> Result<T, SessionError> {
    if Instant::now() >= deadline { return Err(job_timeout(unit)); }
    future::or(operation, async {
        async_io::Timer::at(deadline).await;
        Err(job_timeout(unit))
    }).await
}

fn job_timeout(unit: &str) -> SessionError {
    zbus::Error::from(zbus::fdo::Error::TimedOut(format!("systemd job deadline expired for {unit}"))).into()
}

fn generation(properties: &UnitProperties) -> Result<Option<String>, SessionError> {
    let Some((_, value)) = properties.iter().find(|(name, _)| *name == "Environment") else { return Ok(None); };
    let environment: Vec<String> = Vec::try_from(OwnedValue::try_from(value).map_err(zbus::Error::from)?)
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
        if Instant::now() >= deadline { return Err(job_timeout(unit)); }
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
        async_io::Timer::at((Instant::now() + Duration::from_millis(50)).min(deadline)).await;
    }
}

async fn inspect(manager: &UserManager, unit: &str, operation: &Operation) -> zbus::Result<bool> {
    let path = match manager.proxy().await?.call::<_, _, OwnedObjectPath>("GetUnit", &(unit,)).await {
        Ok(path) => path,
        Err(error) if missing_unit(&error) => return Ok(matches!(operation, Operation::Stop)),
        Err(error) => return Err(error),
    };
    let connection = manager.connection();
    let proxy = zbus::proxy::Builder::<Proxy<'_>>::new(&connection).destination(DESTINATION)?.path(path.clone())?
        .interface("org.freedesktop.systemd1.Unit")?.cache_properties(zbus::proxy::CacheProperties::No).build().await?;
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
    let proxy = zbus::proxy::Builder::<Proxy<'_>>::new(&connection).destination(DESTINATION)?.path(path)?
        .interface("org.freedesktop.systemd1.Service")?.cache_properties(zbus::proxy::CacheProperties::No).build().await?;
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
    while let Some(Some(message)) = future::poll_once(signals.next()).await {
        if job_message(message, job)? { return Ok(true); }
    }
    Ok(false)
}

#[cfg(test)]
#[path = "bus_jobs_tests.rs"]
mod tests;

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
    let new_unit = match retry_read(|| async {
        manager.proxy().await?.call::<_, _, OwnedObjectPath>("GetUnit", &(unit,)).await
    }, Duration::from_secs(5)).await {
        Ok(_) => false,
        Err(error) if missing_unit(&error) => true,
        Err(error) => return Err(error.into()),
    };
    let generation = generation(properties)?;
    let proxy = manager.proxy().await?;
    let mut signals = proxy.receive_signal_with_args("JobRemoved", &[(2, unit)]).await?;
    let auxiliary: Vec<(&str, UnitProperties)> = Vec::new();
    let reply = proxy.call("StartTransientUnit", &(unit, "fail", properties, auxiliary)).await;
    complete(manager, unit, &mut signals, reply, Operation::Start { new_unit, generation }, timeout).await
}

pub(super) async fn stop(manager: &UserManager, unit: &str, timeout: Duration) -> Result<(), SessionError> {
    let proxy = manager.proxy().await?;
    let mut signals = proxy.receive_signal_with_args("JobRemoved", &[(2, unit)]).await?;
    let reply = proxy.call::<_, _, OwnedObjectPath>("StopUnit", &(unit, "replace")).await;
    if reply.as_ref().is_err_and(missing_unit) { return Ok(()); }
    complete(manager, unit, &mut signals, reply, Operation::Stop, timeout).await
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
    reply: zbus::Result<OwnedObjectPath>, operation: Operation, timeout: Duration,
) -> Result<(), SessionError> {
    let job = match reply {
        Ok(job) => {
            if wait_job(signals, &job, timeout).await? { return Ok(()); }
            Some(job)
        }
        Err(error) if retryable_error(&error) => None,
        Err(error) => return Err(error.into()),
    };
    tracing::warn!(unit, "systemd job completion was not received; checking unit state without repeating the request");
    // A timeout already spent the job budget. Permit a single bounded state
    // query afterwards; an ambiguous method reply can still await its job.
    let budget = if job.is_some() { Duration::from_secs(5) } else { timeout };
    let deadline = Instant::now() + budget;
    future::or(async {
        loop {
            if let Some(job) = &job {
                if drain_job(signals, job).await? { return Ok(()); }
            }
            let complete = retry_read(|| inspect(manager, unit, &operation), budget).await?;
            if let Some(job) = &job {
                if drain_job(signals, job).await? { return Ok(()); }
            }
            if complete { return Ok(()); }
            if job.is_some() { break; }
            async_io::Timer::after(Duration::from_millis(50)).await;
        }
        Err(SessionError::State(format!("{unit} job completion could not be confirmed")))
    }, async {
        async_io::Timer::at(deadline).await;
        Err(SessionError::State(format!("timed out confirming systemd job for {unit}")))
    }).await
}

async fn inspect(manager: &UserManager, unit: &str, operation: &Operation) -> zbus::Result<bool> {
    let path = match manager.proxy().await?.call::<_, _, OwnedObjectPath>("GetUnit", &(unit,)).await {
        Ok(path) => path,
        Err(error) if missing_unit(&error) => return Ok(matches!(operation, Operation::Stop)),
        Err(error) => return Err(error),
    };
    let proxy = zbus::proxy::Builder::<Proxy<'_>>::new(&manager.connection).destination(DESTINATION)?.path(path.clone())?
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
    let proxy = zbus::proxy::Builder::<Proxy<'_>>::new(&manager.connection).destination(DESTINATION)?.path(path)?
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

// False means the result was not observed, not that the job failed.
pub(super) async fn wait_job(
    signals: &mut zbus::proxy::SignalStream<'_>, job: &OwnedObjectPath, timeout: Duration,
) -> Result<bool, SessionError> {
    future::or(async {
        while let Some(message) = signals.next().await {
            if job_message(message, job)? { return Ok(true); }
        }
        Ok(false)
    }, async {
        async_io::Timer::after(timeout).await;
        Ok(false)
    }).await
}

#[cfg(test)]
#[path = "bus_jobs_tests.rs"]
mod tests;

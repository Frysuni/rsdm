//! Keep terminal animation independent of power requests and diagnostic reads.

use std::time::{Duration, Instant};

use anyhow::Result;
use rsdm_core::domain::SessionManagerConfig;
use rsdm_session_manager::{SessionStatusDetails, StopOutcome};
use rsdm_system::shutdown::{self, Job, LogindStatus};

use crate::{output::{self, power::PowerScreen}, power_state::Progress, session::print_outcome};

pub async fn run(action: &str, cfg: &SessionManagerConfig, coordinated: bool) -> Result<()> {
    if !output::stdout_is_decorated() {
        return print_outcome(request(action, cfg, coordinated).await?);
    }
    let Ok(mut screen) = PowerScreen::open() else {
        return print_outcome(request(action, cfg, coordinated).await?);
    };
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut progress = Progress::new();
    screen.draw(action, &progress)?;
    if !coordinated { progress.phase = "Waiting for logind authorization".into(); }

    let result = watch(action, cfg, coordinated, &mut screen, &mut progress, &mut terminate).await;
    drop(screen);
    let _ = output::power::summary(&progress).stdout();
    let (outcome, note) = result?;
    if let Some(outcome) = outcome { print_outcome(outcome)?; }
    if let Some(note) = note {
        output::notice("POWER MONITOR", format!("{note}\n"), output::WARNING).stdout()?;
    }
    Ok(())
}

async fn request(action: &str, cfg: &SessionManagerConfig, coordinated: bool) -> Result<StopOutcome> {
    if coordinated { return Ok(rsdm_session_manager::stop(action, cfg).await?); }
    rsdm_system::logind::request_direct(action).await?;
    Ok(StopOutcome { result: "accepted".into(), forced_units: Vec::new(), message: format!("logind accepted {action}") })
}

async fn watch(
    action: &str, cfg: &SessionManagerConfig, coordinated: bool, screen: &mut PowerScreen,
    progress: &mut Progress, terminate: &mut tokio::signal::unix::Signal,
) -> Result<(Option<StopOutcome>, Option<&'static str>)> {
    let (initial_sender, mut initial) = tokio::sync::oneshot::channel();
    let operation = async {
        if coordinated { let _ = initial_sender.send(session_status(cfg).await); }
        request(action, cfg, coordinated).await
    };
    tokio::pin!(operation);
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let mut initialized = !coordinated;
    let mut sampling = Box::pin(sample(cfg, false, Duration::ZERO));
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut outcome = None;
    let mut accepted_at = None;
    loop {
        tokio::select! {
            biased;
            baseline = &mut initial, if !initialized => {
                match baseline? {
                    Ok(details) => progress.initialize(details),
                    Err(error) => progress.session_error = Some(error),
                }
                initialized = true;
            }
            result = &mut operation, if outcome.is_none() => {
                let result = result?;
                if !matches!(result.result.as_str(), "accepted" | "completed" | "prepared" | "delegated") {
                    return Ok((Some(result), None));
                }
                progress.outcome = Some(format!("{}: {}", result.result, result.message));
                outcome = Some(result);
                accepted_at = Some(Instant::now());
            }
            _ = &mut interrupt => return Ok((outcome, Some("Stopped watching; a dispatched power request may still continue."))),
            _ = terminate.recv() => return Ok((outcome, Some("Power monitor stopped by a termination signal; shutdown may still continue."))),
            snapshot = &mut sampling => {
                if let Some(session) = snapshot.session {
                    match session {
                        Ok(details) => progress.session(details),
                        Err(error) => progress.session_error = Some(error),
                    }
                }
                progress.system(snapshot.jobs, snapshot.logind);
                sampling = Box::pin(sample(cfg, coordinated && initialized && outcome.is_none(), Duration::from_secs(1)));
            }
            _ = tick.tick() => {
                if screen.draw(action, progress).is_err() {
                    let result = match outcome { Some(result) => result, None => operation.await? };
                    return Ok((Some(result), Some("Terminal output unavailable; stopped watching.")));
                }
                if let Some(since) = accepted_at
                    && let Some(note) = monitor_finished(progress, since)
                {
                    return Ok((outcome, Some(note)));
                }
            }
        }
    }
}

fn monitor_finished(progress: &Progress, accepted_at: Instant) -> Option<&'static str> {
    if progress.system_errors.len() == 2 {
        return Some("System shutdown status unavailable; the power request is not retried. See the system console for later shutdown stages.");
    }
    let queued = progress.jobs.values().any(|job| job.queued);
    let preparing = progress.logind.as_ref().is_some_and(|status| status.preparing);
    if progress.observed_shutdown && !preparing && !queued && progress.system_errors.is_empty() {
        return Some("Shutdown is no longer reported as active; it may have been cancelled. No request was retried.");
    }
    if !progress.observed_shutdown && accepted_at.elapsed() >= Duration::from_secs(5) {
        return Some("Power request handed off; no active system shutdown is visible. Check native desktop dialogs or the system console.");
    }
    None
}

struct Sample {
    session: Option<Result<SessionStatusDetails, String>>,
    jobs: Result<Vec<Job>, String>,
    logind: Result<LogindStatus, String>,
}

async fn sample(cfg: &SessionManagerConfig, coordinated: bool, delay: Duration) -> Sample {
    // shortcut: polling can miss short jobs; use JobRemoved signals if complete job history is needed.
    tokio::time::sleep(delay).await;
    let (session, (jobs, logind)) = tokio::join!(async {
        if coordinated { Some(session_status(cfg).await) } else { None }
    }, system_status());
    Sample { session, jobs, logind }
}

async fn session_status(cfg: &SessionManagerConfig) -> Result<SessionStatusDetails, String> {
    tokio::time::timeout(Duration::from_secs(2), rsdm_session_manager::status_details(cfg)).await
        .map_err(|_| "session status timed out".to_string())?.map_err(|error| error.to_string())
}

async fn system_status() -> (Result<Vec<Job>, String>, Result<LogindStatus, String>) {
    let connection = match shutdown::connect().await {
        Ok(connection) => connection,
        Err(error) => return (Err(error.to_string()), Err(error.to_string())),
    };
    let (jobs, logind) = tokio::join!(shutdown::jobs(&connection), shutdown::logind_status(&connection));
    (jobs.map_err(|error| error.to_string()), logind.map_err(|error| error.to_string()))
}

#[cfg(test)]
#[path = "power_monitor_tests.rs"]
mod tests;

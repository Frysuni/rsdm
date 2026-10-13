//! Recovery is restricted to recorded invocations of one login generation.

use std::time::Duration;

use zbus::fdo::RequestNameFlags;

use super::{
    SessionError, app_stop::{self, ShutdownControl}, apps, bus::UserManager,
    control::BUS_NAME, deadline::Deadline, env, identity::GENERATION_ENV,
    runtime::{Runtime, SessionRecord}, units,
};

pub async fn cleanup(generation: &str, actions: super::action_gate::ActionGate) -> Result<(), SessionError> {
    let generation = generation.to_string();
    let Some((runtime, _lease, saved_deadline)) = super::runtime::blocking(move || {
        open_recovery(&generation)
    }).await? else { return Ok(()); };
    let actions = actions.with_generation(runtime.clone());
    let deadline = Deadline::default();
    deadline.set(recovery_deadline(saved_deadline, 0)?);
    let result = async {
        let manager = UserManager::connect_guarded(deadline, actions.clone()).await?;
        manager.actions.guard(acquire_lease(&manager)).await??;
        recover(&manager, &runtime).await
    }.await;
    if actions.check().is_err() {
        super::runtime::blocking(move || {
            if runtime.is_disabled()? { return Ok(()); }
            let mut record = runtime.session()?;
            record.disabled = true;
            runtime.save_session(&record)
        }).await?;
        return Ok(());
    }
    result
}

fn open_recovery(generation: &str) -> Result<Option<(Runtime, super::session_lease::SessionLease, Option<u64>)>, SessionError> {
    let Some(runtime) = Runtime::open(generation)? else { return Ok(None); };
    if runtime.is_disabled()? { return Ok(None); }
    // Validation may have created the private directory before publishing a
    // session record. No service or environment side effect precedes that
    // record, so an empty generation needs no user-manager connection.
    if !runtime.path.join("session.toml").try_exists()? && runtime.apps()?.is_empty() {
        return Ok(None);
    }
    let record = runtime.session()?;
    if record.disabled { return Ok(None); }
    let lease = super::session_lease::SessionLease::acquire()?;
    let record = runtime.session()?;
    if record.disabled { return Ok(None); }
    Ok(Some((runtime, lease, record.shutdown_deadline_usec)))
}

pub(super) async fn recover(manager: &UserManager, runtime: &Runtime) -> Result<(), SessionError> {
    let records = runtime.clone();
    if super::runtime::blocking(move || records.is_disabled()).await? { return Ok(()); }
    let records = runtime.clone();
    let mut record = super::runtime::blocking(move || records.session()).await?;
    if record.disabled { return Ok(()); }
    let mut control = ShutdownControl::for_manager(manager);
    control.transaction_id = record.shutdown_transaction.as_ref().map(|transaction| transaction.id.clone());
    let control = std::sync::Arc::new(control);
    control.noncancelable.store(true, std::sync::atomic::Ordering::SeqCst);
    control.recovery.store(true, std::sync::atomic::Ordering::SeqCst);
    let deadline = recovery_deadline(record.shutdown_deadline_usec, manager.deadline.get())?;
    control.force(deadline);
    record.shutdown_deadline_usec = Some(deadline);

    let mut failure = None;
    let records = runtime.clone();
    let recovering = record.clone();
    if let Err(error) = super::runtime::blocking(move || records.save_session(&recovering)).await { failure = Some(error); }
    let records = runtime.clone();
    let applications = super::runtime::blocking(move || records.apps()).await?;
    if let Err(error) = app_stop::prepare_apps(&manager, &runtime, applications, control).await {
        failure = Some(error);
    }

    let mut cleanup_manager = manager.clone();
    cleanup_manager.deadline = manager.deadline.reserving(Duration::from_secs(1))?;
    let records = runtime.clone();
    for app in super::runtime::blocking(move || records.apps()).await? {
        if let Err(error) = apps::finish(&cleanup_manager, &runtime, &app).await { failure.get_or_insert(error); }
    }

    recover_infrastructure(manager, &cleanup_manager, runtime, &mut record, &mut failure).await;
    let records = runtime.clone();
    if let Err(error) = super::runtime::blocking(move || records.reap_closed_app_locks()).await { failure.get_or_insert(error); }
    if let Some(error) = failure { return Err(error); }

    record.phase = rsdm_core::domain::SessionPhase::Closed;
    let records = runtime.clone();
    super::runtime::blocking(move || records.save_session(&record)).await
}

async fn recover_infrastructure(
    manager: &UserManager, cleanup_manager: &UserManager, runtime: &Runtime,
    record: &mut SessionRecord, failure: &mut Option<SessionError>,
) {
    if record.provider == "partial" { return; }
    if let Err(error) = verify_compositor(manager, runtime, record).await {
        failure.get_or_insert(error);
        return;
    }
    let manager = match &record.compositor_unit {
        Some(unit) => manager.with_owner(unit, &record.compositor_invocation),
        None => manager.clone(),
    };
    let mut cleanup_manager = cleanup_manager.clone();
    if let Some(unit) = &record.compositor_unit {
        cleanup_manager = cleanup_manager.with_owner(unit, &record.compositor_invocation);
    }

    let owns_environment = match cleanup_manager.environment().await {
        Ok(environment) => environment.contains(&format!("{GENERATION_ENV}={}", runtime.generation)),
        Err(error) => { failure.get_or_insert(error); false }
    };
    if record.owns_targets && owns_environment {
        for target in [units::AUTOSTART_TARGET, units::SESSION_TARGET, units::PRE_TARGET] {
            if let Err(error) = cleanup_manager.stop(target, Duration::from_secs(15)).await { failure.get_or_insert(error); }
        }
    }
    if let Err(error) = cleanup_manager.stop(&record.anchor_unit, Duration::from_secs(15)).await { failure.get_or_insert(error); }
    let _ = cleanup_manager.unref(&record.anchor_unit).await;
    if let Some(unit) = record.compositor_unit.clone() {
        if let Err(error) = super::processes::stop_invocation(&manager, &unit, &record.compositor_invocation).await { failure.get_or_insert(error); }
        if record.provider == "managed" { let _ = manager.unref(&unit).await; }
    }
    if owns_environment {
        if let Err(error) = env::clear_owned(&manager, &record.exported_environment).await { failure.get_or_insert(error); }
    }
}

async fn verify_compositor(manager: &UserManager, runtime: &Runtime, record: &mut SessionRecord) -> Result<(), SessionError> {
    let Some(unit) = &record.compositor_unit else { return Ok(()); };
    if record.compositor_invocation.is_empty() && record.provider == "managed" {
        let Some(id) = super::processes::generation_invocation(manager, unit, &runtime.generation).await? else { return Ok(()); };
        record.compositor_invocation = id;
    }
    super::processes::verify_unit_invocation(manager, unit, &record.compositor_invocation).await?;
    Ok(())
}

pub(super) fn recovery_deadline(saved: Option<u64>, current: u64) -> Result<u64, SessionError> {
    let mut deadline = match saved {
        Some(saved) => saved,
        None => super::processes::monotonic_usec()?.saturating_add(5_000_000),
    };
    if current != 0 { deadline = deadline.min(current); }
    Ok(deadline)
}

async fn acquire_lease(manager: &UserManager) -> Result<(), SessionError> {
    manager.deadline.bound(async {
        let lease = async {
            loop {
                match manager.connection().request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into()).await {
                    Ok(_) => return Ok(()),
                    Err(zbus::Error::NameTaken) => { tokio::time::sleep(Duration::from_millis(50)).await; }
                    Err(error) => return Err(error.into()),
                }
            }
        };
        tokio::select! {
            biased;
            result = lease => result,
            _ = tokio::time::sleep(Duration::from_secs(2)) => {
                Err(zbus::Error::from(zbus::fdo::Error::TimedOut("acquiring the recovery bus name timed out".into())).into())
            },
        }
    }).await
}

#[cfg(test)]
#[path = "cleanup_tests.rs"]
mod tests;

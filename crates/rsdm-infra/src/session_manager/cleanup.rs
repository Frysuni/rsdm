//! Recovery is restricted to recorded invocations of one login generation.

use std::{thread, time::{Duration, Instant}};

use zbus::fdo::RequestNameFlags;

use super::{
    SessionError, app_stop::{self, ShutdownControl}, apps, bus::UserManager,
    control::BUS_NAME, env, identity::GENERATION_ENV,
    runtime::Runtime, units,
};

pub fn cleanup(generation: &str) -> Result<(), SessionError> {
    let Some(runtime) = Runtime::open(generation)? else { return Ok(()); };
    // Validation may have created the private directory before publishing a
    // session record. No service or environment side effect precedes that
    // record, so an empty generation needs no user-manager connection.
    if !runtime.path.join("session.toml").try_exists()? && runtime.apps()?.is_empty() {
        return Ok(());
    }
    let _lease = super::session_lease::SessionLease::acquire()?;
    let manager = UserManager::connect()?;
    acquire_lease(&manager)?;
    recover(&manager, &runtime)
}

pub(super) fn recover(manager: &UserManager, runtime: &Runtime) -> Result<(), SessionError> {
    let mut record = runtime.session()?;
    let control = std::sync::Arc::new(ShutdownControl::default());
    control.noncancelable.store(true, std::sync::atomic::Ordering::SeqCst);
    control.recovery.store(true, std::sync::atomic::Ordering::SeqCst);
    let recovery_deadline = super::processes::monotonic_usec()?.saturating_add(5_000_000);
    let deadline = record.shutdown_deadline_usec.map_or(recovery_deadline, |saved| saved.min(recovery_deadline));
    control.force(deadline);
    record.shutdown_deadline_usec = Some(deadline);
    let mut failure = None;
    if let Err(error) = runtime.save_session(&record) { failure = Some(error); }
    if let Err(error) = app_stop::prepare_apps(&manager, &runtime, runtime.apps()?, control) {
        failure = Some(error);
    }
    for app in runtime.apps()? {
        if let Err(error) = apps::finish(&manager, &runtime, &app) { failure.get_or_insert(error); }
    }
    let owns_environment = manager.environment()?.contains(&format!("{GENERATION_ENV}={}", runtime.generation));
    if record.owns_targets && owns_environment {
        for target in [units::AUTOSTART_TARGET, units::SESSION_TARGET, units::PRE_TARGET] {
            if let Err(error) = manager.stop(target, Duration::from_secs(15)) { failure.get_or_insert(error); }
        }
    }
    if let Err(error) = manager.stop(&record.anchor_unit, Duration::from_secs(15)) { failure.get_or_insert(error); }
    let _ = manager.unref(&record.anchor_unit);
    if let Some(unit) = &record.compositor_unit {
        let stopped = (|| {
            if record.compositor_invocation.is_empty() && record.provider == "managed" {
                if let Some(id) = super::processes::generation_invocation(manager, unit, &runtime.generation)? {
                    record.compositor_invocation = id;
                } else { return Ok(()); }
            }
            super::processes::stop_invocation(manager, unit, &record.compositor_invocation)
        })();
        if let Err(error) = stopped { failure.get_or_insert(error); }
        if record.provider == "managed" { let _ = manager.unref(unit); }
    }
    if owns_environment {
        if let Err(error) = env::clear_owned(&manager, &record.exported_environment) { failure.get_or_insert(error); }
    }
    if let Some(error) = failure { return Err(error); }
    record.phase = rsdm_core::domain::SessionPhase::Closed;
    runtime.save_session(&record)
}

fn acquire_lease(manager: &UserManager) -> Result<(), SessionError> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let result = async_io::block_on(manager.connection.request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into()));
        match result {
            Ok(_) => return Ok(()),
            Err(zbus::Error::NameTaken) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(error.into()),
        }
    }
}

//! Initial environment publication with a recovery record before side effects.

use super::{SessionError, bus::UserManager, env, runtime::{Runtime, SessionRecord}};

pub(super) async fn publish(manager: &UserManager, runtime: &Runtime, record: &SessionRecord) -> Result<(), SessionError> {
    let records = runtime.clone();
    let initial = record.clone();
    super::runtime::blocking(move || records.save_session(&initial)).await?;
    let result = async {
        env::clear_names(manager, &env::DISPLAY_VARS.iter().map(|name| name.to_string()).collect::<Vec<_>>()).await?;
        env::publish(manager, &record.exported_environment).await
    }.await;
    if result.is_err() { rollback(manager, runtime, record).await; }
    result
}

pub(super) async fn rollback(manager: &UserManager, runtime: &Runtime, record: &SessionRecord) {
    if manager.actions.check().is_err() {
        record_disabled(runtime, record).await;
        return;
    }
    if let Err(error) = env::clear_owned(manager, &record.exported_environment).await {
        if manager.actions.check().is_err() {
            record_disabled(runtime, record).await;
        }
        tracing::warn!(%error, "startup environment rollback failed");
        return;
    }
    let mut closed = record.clone();
    closed.phase = rsdm_core::domain::SessionPhase::Closed;
    let runtime = runtime.clone();
    if let Err(error) = super::runtime::blocking(move || runtime.save_session(&closed)).await {
        tracing::warn!(%error, "could not record startup rollback");
    }
}

async fn record_disabled(runtime: &Runtime, record: &SessionRecord) {
    let mut disabled = record.clone();
    disabled.disabled = true;
    let runtime = runtime.clone();
    if let Err(error) = super::runtime::blocking(move || runtime.save_session(&disabled)).await {
        tracing::error!(%error, "could not record disabled startup");
    }
}

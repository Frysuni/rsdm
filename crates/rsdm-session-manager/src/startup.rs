//! Initial environment publication with a recovery record before side effects.

use super::{SessionError, bus::UserManager, env, runtime::{Runtime, SessionRecord}};

pub(super) async fn publish(manager: &UserManager, runtime: &Runtime, record: &SessionRecord) -> Result<(), SessionError> {
    runtime.save_session(record)?;
    let result = async {
        env::clear_names(manager, &env::DISPLAY_VARS.iter().map(|name| name.to_string()).collect::<Vec<_>>()).await?;
        env::publish(manager, &record.exported_environment).await
    }.await;
    if result.is_err() { rollback(manager, runtime, record).await; }
    result
}

pub(super) async fn rollback(manager: &UserManager, runtime: &Runtime, record: &SessionRecord) {
    if let Err(error) = env::clear_owned(manager, &record.exported_environment).await {
        tracing::warn!(%error, "startup environment rollback failed");
        return;
    }
    let mut closed = record.clone();
    closed.phase = rsdm_core::domain::SessionPhase::Closed;
    if let Err(error) = runtime.save_session(&closed) {
        tracing::warn!(%error, "could not record startup rollback");
    }
}

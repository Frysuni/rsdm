//! Independent hooks consume later reductions of the generation's budget.

use std::{future::Future, time::Duration};

use crate::{SessionError, deadline::Deadline, runtime::{Runtime, blocking}};

pub(super) async fn follow<T>(
    runtime: &Runtime, deadline: &Deadline, operation: impl Future<Output = Result<T, SessionError>>,
) -> Result<T, SessionError> {
    tokio::select! {
        result = deadline.bound(operation) => result,
        error = observe(runtime, deadline) => Err(error),
    }
}

async fn observe(runtime: &Runtime, deadline: &Deadline) -> SessionError {
    loop {
        let records = runtime.clone();
        let record = match blocking(move || records.session()).await {
            Ok(record) => record,
            Err(error) => return error,
        };
        if let Some(saved) = record.shutdown_deadline_usec {
            let current = deadline.get();
            if current == 0 || saved < current { deadline.set(saved); }
        }
        // A stopped systemd unit cannot revoke its accepted ExecStop. A later
        // transaction or revoked session deadline must not extend this hook.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
#[path = "runtime_deadline_tests.rs"]
mod tests;

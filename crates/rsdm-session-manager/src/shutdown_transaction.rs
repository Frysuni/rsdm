//! Durable identity and original budget for one shutdown preparation.

use serde::{Deserialize, Serialize};

use super::{SessionError, processes::monotonic_usec, runtime::{Runtime, blocking}};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ShutdownTransaction {
    pub(super) id: String,
    pub(super) action: String,
    pub(super) started_usec: u64,
    pub(super) deadline_usec: u64,
    pub(super) result: Option<String>,
}

impl ShutdownTransaction {
    pub(super) async fn start(runtime: &Runtime, action: &str, proposed: Option<u64>) -> Result<Self, SessionError> {
        let started_usec = monotonic_usec()?;
        let runtime = runtime.clone();
        let (id, deadline_usec) = blocking(move || {
            let deadline = match proposed {
                Some(deadline) => deadline,
                None => {
                    let timeout = runtime.apps()?.iter().map(|app| app.policy.timeout_secs).max().unwrap_or(30);
                    let budget = timeout.checked_add(90).and_then(|seconds| seconds.checked_mul(1_000_000));
                    budget.and_then(|budget| started_usec.checked_add(budget))
                        .ok_or_else(|| SessionError::State("shutdown transaction deadline overflow".into()))?
                }
            };
            Ok((super::identity::new_generation()?, deadline))
        }).await?;
        Ok(Self { id, action: action.into(), started_usec, deadline_usec, result: None })
    }

    pub(super) fn shorten(&mut self, proposed: u64) {
        self.deadline_usec = self.deadline_usec.min(proposed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortening_a_saved_transaction_never_replaces_its_identity_or_start() {
        let mut transaction = ShutdownTransaction { id: "original".into(), action: "logout".into(),
            started_usec: 10, deadline_usec: 100, result: None };
        transaction.shorten(200);
        assert_eq!(transaction.deadline_usec, 100);
        transaction.shorten(50);
        assert_eq!(transaction.deadline_usec, 50);
        assert_eq!(transaction.id, "original");
        assert_eq!(transaction.started_usec, 10);
    }
}

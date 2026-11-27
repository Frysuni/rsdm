//! Durable application policy, ownership and preparation state.

use rsdm_core::domain::ShutdownPolicy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AppRecord {
    pub unit: String,
    pub invocation_id: Vec<u8>,
    pub original_process: Option<super::app_origin::OriginalProcess>,
    pub migrated_scope: Option<super::app_scope::MigratedScope>,
    pub lifetime_process: Option<super::app_lifetime::LifetimeProcess>,
    pub policy: ShutdownPolicy,
    pub deadline_usec: Option<u64>,
    pub quit_started: bool,
    pub dispatch: Option<super::app_dispatch::Dispatch>,
    pub window_close_sent: bool,
    pub preparation: Option<super::app_result::Preparation>,
}

impl AppRecord {
    pub(super) fn validate(&self) -> Result<(), super::SessionError> {
        self.policy.validate().map_err(|error| super::SessionError::State(error.into()))?;
        if self.quit_started && self.dispatch.is_none() {
            return Err(super::SessionError::State("application dispatch record is missing".into()));
        }
        Ok(())
    }
}

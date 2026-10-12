//! Durable application policy, ownership and preparation state.

use rsdm_core::domain::ShutdownPolicy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AppRecord {
    pub unit: String,
    pub invocation_id: Vec<u8>,
    #[serde(default)]
    pub original_process: Option<super::app_origin::OriginalProcess>,
    #[serde(default)]
    pub migrated_scope: Option<super::app_scope::MigratedScope>,
    #[serde(default)]
    pub lifetime_process: Option<super::app_lifetime::LifetimeProcess>,
    pub policy: ShutdownPolicy,
    pub deadline_usec: Option<u64>,
    pub quit_started: bool,
    #[serde(default)]
    pub dispatch: Option<super::app_dispatch::Dispatch>,
    #[serde(default)]
    pub window_close_sent: bool,
    #[serde(default)]
    pub preparation: Option<super::app_result::Preparation>,
}

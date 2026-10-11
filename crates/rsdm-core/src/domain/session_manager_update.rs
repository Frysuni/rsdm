use super::SessionManagerConfig;

/// Validated policy with a retained history of disable transitions. A watch
/// receiver may skip intermediate values, but an old generation must still stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionManagerUpdate {
    pub config: SessionManagerConfig,
    pub disable_count: u64,
}

impl From<SessionManagerConfig> for SessionManagerUpdate {
    fn from(config: SessionManagerConfig) -> Self {
        Self { config, disable_count: 0 }
    }
}

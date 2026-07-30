mod design;
mod dm;
mod idle;
mod lock;
mod session;
mod session_manager;
mod system;
mod validation;

use serde::{Deserialize, Serialize};

pub use design::*;
pub use dm::*;
pub use idle::*;
pub use lock::*;
pub use session::*;
pub use session_manager::*;
pub use system::*;
pub use validation::*;

/// The whole rsdm configuration.
///
/// Top level carries only the cross-cutting settings (session manager, logging,
/// security, paths). Everything specific to a front lives under [`Self::dm`] (the
/// greeter), [`Self::lock`] (the locker), or [`Self::idle`] (idle supervision),
/// and the look of each visual front lives under its `design`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub session_manager: SessionManagerConfig,
    pub logging: LoggingConfig,
    pub security: SecurityConfig,
    pub paths: PathsConfig,
    pub dm: DmConfig,
    pub lock: LockConfig,
    pub idle: IdleConfig,
}

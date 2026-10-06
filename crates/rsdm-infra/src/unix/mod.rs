mod command;
mod fallback;
mod getty_query;
mod group;
mod launcher;
mod session_environment;
mod session_leader;
mod session_conversation;
mod session_pipe;
mod session_report;
mod shutdown;
mod user;
mod vt;
mod vt_owner;

pub use fallback::{FallbackError, exec_fallback};
pub use getty_query::discover_system_getty;
pub use launcher::UnixSessionLauncher;
pub use session_leader::{
    LeaderGate, LeaderHandle, LeaderLaunch, LeaderReport, spawn_session_leader,
};
pub use shutdown::{
    emergency_unlock_requested, install_emergency_unlock_handler, install_terminate_handler,
    terminate_flag, terminate_requested,
};
pub use user::UnixUserResolver;
pub use vt::{VtError, VtGuard, acquire as acquire_vt};

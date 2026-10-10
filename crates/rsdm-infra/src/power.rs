//! Logind adapters used by the DM and PAM session owner.

use std::os::fd::OwnedFd;

pub use rsdm_system::logind::{LogindError, PowerCapabilities, capabilities, request_direct};

/// The PAM owner holds this guard until pam_close_session finishes.
/// Call after fork and before releasing the session child's start gate.
pub fn pam_shutdown_guard() -> Result<OwnedFd, LogindError> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(rsdm_system::logind::delay_inhibitor("Closing the graphical PAM session"))
}

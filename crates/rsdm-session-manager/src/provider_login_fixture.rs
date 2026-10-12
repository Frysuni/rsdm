//! Native delegation tests use a separate, authenticated logind peer.

use super::*;
use crate::control_test_authority::Context;

pub(super) async fn login() -> Arc<Context> {
    let mut identity = crate::helper_start_fixture::record().identity;
    // SAFETY: geteuid has no preconditions.
    identity.uid = unsafe { libc::geteuid() };
    Context::create(identity).await
}

pub(super) async fn delegate(provider: &Provider, manager: &UserManager, action: &str) -> Result<(), SessionError> {
    let login = login().await;
    let identity = login.runtime.session().unwrap().identity;
    provider.delegate_to_login(manager, action, &identity, &login.logind).await
}

pub(super) async fn available(provider: &Provider, manager: &UserManager) -> Result<bool, SessionError> {
    let login = login().await;
    let identity = login.runtime.session().unwrap().identity;
    provider.logout_available_for_login(manager, &identity, &login.logind).await
}

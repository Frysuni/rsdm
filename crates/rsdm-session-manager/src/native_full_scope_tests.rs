//! Full native requests remain tied to the generation's original login.

use super::*;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn foreign_missing_and_unpinned_logins_cannot_activate_or_receive_native_requests() {
    for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
        for invalid in ["login", "uid", "missing", "numeric"] {
            let login = login_fixture::login().await;
            let identity = login.runtime.session().unwrap().identity;
            let mut broker = if invalid == "numeric" { Broker::current() } else { Broker::pinned() };
            let activation = Arc::new(Activation { calls: Default::default(), lost: false, replace_desktop: false });
            if kind == ProviderKind::Plasma {
                broker.missing = true;
                broker.activatable = true;
                broker.activation = Some(activation.clone());
            }
            if matches!(invalid, "login" | "uid") {
                let mut changed = identity.clone();
                if invalid == "login" { changed.login_session_id.push_str("-foreign"); } else { changed.uid ^= 1; }
                login.set_login(changed).await;
            }
            if invalid == "missing" { login.missing_login.store(true, Ordering::SeqCst); }
            let (manager, _server, state) = fixture(Reply::Success, broker).await;
            let provider = Provider { kind, native_unit: None, logout_command: Vec::new() };
            assert!(provider.logout_available_for_login(&manager, &identity, &login.logind).await.is_err(), "{kind:?}: {invalid}");
            assert!(provider.delegate_to_login(&manager, "logout", &identity, &login.logind).await.is_err(), "{kind:?}: {invalid}");
            assert!(state.calls.lock().unwrap().is_empty());
            assert_eq!(activation.calls.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn login_revocation_while_waiting_for_request_slots_prevents_native_logout() {
    let login = login_fixture::login().await;
    let identity = login.runtime.session().unwrap().identity;
    let (manager, _server, state) = fixture(Reply::Success, Broker::pinned()).await;
    let (entered, mut occupied) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let manager = manager.clone();
        let entered = entered.clone();
        let release = release.clone();
        workers.spawn(async move { manager.request(async {
            entered.send(()).unwrap();
            release.acquire().await.unwrap().forget();
            Ok(())
        }).await.unwrap(); });
    }
    for _ in 0..4 { tokio::time::timeout(Duration::from_secs(1), occupied.recv()).await.unwrap().unwrap(); }
    let provider = Provider { kind: ProviderKind::Gnome, native_unit: None, logout_command: Vec::new() };
    let request = provider.delegate_to_login(&manager, "logout", &identity, &login.logind);
    tokio::pin!(request);
    assert!(tokio::time::timeout(Duration::from_millis(25), &mut request).await.is_err());
    let mut changed = identity.clone();
    changed.login_session_id.push_str("-foreign");
    login.set_login(changed).await;
    release.add_permits(1);
    assert!(tokio::time::timeout(Duration::from_secs(1), &mut request).await.unwrap().is_err());
    release.add_permits(3);
    while let Some(result) = workers.join_next().await { result.unwrap(); }
    assert!(state.calls.lock().unwrap().is_empty());
}

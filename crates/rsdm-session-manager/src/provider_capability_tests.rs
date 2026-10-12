//! Native capability queries authenticate peers without requesting shutdown or activation.

use std::sync::atomic::Ordering;

use super::*;

#[tokio::test]
async fn owned_native_services_are_reported_without_shutdown_requests() {
    for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
        let (manager, _server, state) = fixture(Reply::Success, Broker::current()).await;
        let provider = Provider { kind, native_unit: None, logout_command: Vec::new() };
        assert!(provider.logout_available(&manager).await.unwrap());
        assert!(state.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn an_activatable_plasma_prompt_is_reported_without_starting_it() {
    let activation = Arc::new(Activation { calls: Default::default(), lost: false, replace_desktop: false });
    let mut broker = Broker::current();
    broker.missing = true;
    broker.activatable = true;
    broker.activation = Some(activation.clone());
    let (manager, _server, state) = fixture(Reply::Success, broker).await;
    let provider = Provider { kind: ProviderKind::Plasma, native_unit: None, logout_command: Vec::new() };
    assert!(provider.logout_available(&manager).await.unwrap());
    assert_eq!(activation.calls.load(Ordering::SeqCst), 0);
    assert!(state.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn activation_metadata_requires_a_live_desktop_and_an_available_prompt() {
    for desktop in [false, true] {
        let activation = Arc::new(Activation { calls: Default::default(), lost: false, replace_desktop: false });
        let mut broker = Broker::current();
        broker.missing = true;
        broker.activatable = !desktop;
        if desktop { broker.activation = Some(activation.clone()); }
        let (manager, _server, state) = fixture(Reply::Success, broker).await;
        let provider = Provider { kind: ProviderKind::Plasma, native_unit: None, logout_command: Vec::new() };
        assert!(!provider.logout_available(&manager).await.unwrap());
        assert_eq!(activation.calls.load(Ordering::SeqCst), 0);
        assert!(state.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn missing_foreign_unidentified_and_replaced_peers_are_not_available() {
    for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
        for invalid in ["missing", "foreign", "no-uid", "no-pid", "zero-pid", "changed", "changed-desktop"] {
            if kind == ProviderKind::Gnome && invalid == "changed-desktop" { continue; }
            let mut broker = Broker::current();
            match invalid {
                "missing" => broker.missing = true,
                "foreign" => broker.uid = broker.uid.map(|uid| uid ^ 1),
                "no-uid" => broker.uid = None,
                "no-pid" => broker.pid = None,
                "zero-pid" => broker.pid = Some(0),
                "changed" => broker.replaced = true,
                "changed-desktop" => broker.replaced_desktop = true,
                _ => unreachable!(),
            }
            let (manager, _server, state) = fixture(Reply::Success, broker).await;
            let provider = Provider { kind, native_unit: None, logout_command: Vec::new() };
            let result = provider.logout_available(&manager).await;
            assert!(!matches!(result, Ok(true)), "{kind:?}: {invalid}");
            assert!(state.calls.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn disable_and_expired_deadlines_keep_native_probes_passive() {
    for disabled in [false, true] {
        let (manager, _server, state) = fixture(Reply::Success, Broker::current()).await;
        if disabled { manager.actions.close(); } else { manager.deadline.set(1); }
        let provider = Provider { kind: ProviderKind::Plasma, native_unit: None, logout_command: Vec::new() };
        assert!(provider.logout_available(&manager).await.is_err());
        assert!(state.calls.lock().unwrap().is_empty());
    }
}

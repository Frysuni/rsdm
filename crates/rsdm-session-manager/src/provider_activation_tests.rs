//! Plasma's on-demand helper is activated only behind a verified live desktop.

use std::sync::atomic::Ordering;

use super::*;

#[tokio::test]
async fn an_absent_plasma_prompt_is_activated_then_addressed_by_its_verified_unique_owner() {
    for action in ["logout", "reboot", "poweroff"] {
        let activation = Arc::new(Activation { calls: Default::default(), lost: false, replace_desktop: false });
        let mut broker = Broker::current();
        broker.missing = true;
        broker.activation = Some(activation.clone());
        let (manager, _server, state) = fixture(Reply::Success, broker).await;
        let provider = Provider { kind: ProviderKind::Plasma, native_unit: None, logout_command: Vec::new() };
        provider.delegate(&manager, action).await.unwrap();
        assert_eq!(activation.calls.load(Ordering::SeqCst), 1);
        assert_eq!(state.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn foreign_desktops_lost_activation_replies_and_replaced_desktops_cannot_dispatch() {
    for invalid in ["foreign", "lost", "changed"] {
        let activation = Arc::new(Activation { calls: Default::default(), lost: invalid == "lost", replace_desktop: invalid == "changed" });
        let mut broker = Broker::current();
        broker.missing = true;
        broker.activation = Some(activation.clone());
        if invalid == "foreign" { broker.uid = broker.uid.map(|uid| uid ^ 1); }
        let (manager, _server, state) = fixture(Reply::Success, broker).await;
        let provider = Provider { kind: ProviderKind::Plasma, native_unit: None, logout_command: Vec::new() };
        assert!(provider.delegate(&manager, "logout").await.is_err());
        assert_eq!(activation.calls.load(Ordering::SeqCst), usize::from(invalid != "foreign"));
        assert!(state.calls.lock().unwrap().is_empty());
    }
}

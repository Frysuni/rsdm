use std::{future::pending, sync::atomic::AtomicBool, time::Duration};

use rsdm_core::domain::SessionManagerConfig;

use super::*;

#[tokio::test]
async fn disabled_policy_never_polls_an_external_action() {
    let config = SessionManagerConfig { enabled: false, ..Default::default() };
    let (_sender, receiver) = watch::channel(SessionManagerUpdate::from(config));
    let actions = ActionGate::new(Some(receiver));
    let polled = AtomicBool::new(false);
    let result = actions.guard(async { polled.store(true, Ordering::SeqCst); }).await;
    assert!(matches!(result, Err(SessionError::Disabled)));
    assert!(!polled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_coalesced_disable_closes_old_generations_and_allows_a_new_attach() {
    let (sender, receiver) = watch::channel(SessionManagerUpdate::from(SessionManagerConfig::default()));
    let old = ActionGate::new(Some(receiver.clone()));
    let clone = old.clone();
    sender.send_modify(|update| { update.config.enabled = false; update.disable_count += 1; });
    sender.send_modify(|update| update.config.enabled = true);

    assert!(matches!(old.check(), Err(SessionError::Disabled)));
    assert!(matches!(clone.check(), Err(SessionError::Disabled)));
    let attached = ActionGate::new(Some(receiver));
    attached.check().unwrap();
    assert!(matches!(old.check(), Err(SessionError::Disabled)));
}

#[tokio::test]
async fn policy_change_interrupts_a_pending_action_without_waiting_for_its_reply() {
    let (sender, receiver) = watch::channel(SessionManagerUpdate::from(SessionManagerConfig::default()));
    let actions = ActionGate::new(Some(receiver));
    let guarded = tokio::spawn(async move { actions.guard(pending::<()>()).await });
    tokio::task::yield_now().await;
    sender.send_modify(|update| { update.config.enabled = false; update.disable_count += 1; });
    let result = tokio::time::timeout(Duration::from_secs(1), guarded).await.unwrap().unwrap();
    assert!(matches!(result, Err(SessionError::Disabled)));
}

#[tokio::test]
async fn losing_the_loader_keeps_the_last_policy_and_explicit_close_wakes_clones() {
    let (sender, receiver) = watch::channel(SessionManagerUpdate::from(SessionManagerConfig::default()));
    let actions = ActionGate::new(Some(receiver));
    drop(sender);
    actions.check().unwrap();
    let clone = actions.clone();
    let guarded = tokio::spawn(async move { clone.guard(pending::<()>()).await });
    tokio::task::yield_now().await;
    actions.close();
    let result = tokio::time::timeout(Duration::from_secs(1), guarded).await.unwrap().unwrap();
    assert!(matches!(result, Err(SessionError::Disabled)));
}

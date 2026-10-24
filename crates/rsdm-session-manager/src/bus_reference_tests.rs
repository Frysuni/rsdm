//! Reference claims must settle independently without losing earlier ownership.

use super::*;

#[tokio::test]
async fn abandoning_an_unsubmitted_intent_removes_only_its_reference() {
    let (connection, _server, _) = fixture(state()).await;
    let owner = Transport::new(connection);
    owner.stage_reference("existing.service", Some("ours".into())).submitted();
    let intent = owner.stage_reference("rejected.service", Some("ours".into()));
    drop(intent);
    assert!(!owner.has_reference("rejected.service"));
    assert!(owner.has_reference("existing.service"));
}

#[tokio::test]
async fn rejecting_a_second_attempt_preserves_the_original_invocation() {
    let (connection, _server, _) = fixture(state()).await;
    let owner = Transport::new(connection);
    owner.stage_reference("example.service", Some("ours".into())).submitted();
    owner.remember("example.service", &[1; 16]);
    let mut retry = owner.stage_reference("example.service", Some("other".into()));
    retry.submitted();
    assert!(retry.reject());
    let state = owner.0.lock().unwrap();
    let reference = &state.references["example.service"];
    assert_eq!(reference.generation.as_deref(), Some("ours"));
    assert_eq!(reference.invocation, Some(vec![1; 16]));
}

#[tokio::test]
async fn overlapping_rejections_remove_the_intent_only_after_both_settle() {
    let (connection, _server, _) = fixture(state()).await;
    let owner = Transport::new(connection);
    let mut first = owner.stage_reference("example.service", Some("ours".into()));
    let mut second = owner.stage_reference("example.service", Some("ours".into()));
    first.submitted();
    second.submitted();
    assert!(first.reject());
    assert!(owner.has_reference("example.service"));
    assert!(second.reject());
    assert!(!owner.has_reference("example.service"));
}

#[tokio::test]
async fn an_accepted_overlap_survives_rejection_in_either_completion_order() {
    for accepted_first in [false, true] {
        let (connection, _server, _) = fixture(state()).await;
        let owner = Transport::new(connection);
        let mut rejected = owner.stage_reference("example.service", Some("ours".into()));
        let mut accepted = Some(owner.stage_reference("example.service", Some("ours".into())));
        rejected.submitted();
        accepted.as_mut().unwrap().submitted();
        if accepted_first { drop(accepted.take()); }
        assert!(rejected.reject());
        drop(accepted);
        assert!(owner.has_reference("example.service"));
        assert_eq!(owner.0.lock().unwrap().references["example.service"].pending, 0);
    }
}

#[tokio::test]
async fn rejecting_an_old_intent_cannot_remove_a_replacement_claim() {
    let (connection, _server, _) = fixture(state()).await;
    let owner = Transport::new(connection);
    let old = owner.stage_reference("example.service", Some("ours".into()));
    owner.forget("example.service");
    let replacement = owner.stage_reference("example.service", Some("new".into()));
    assert!(old.reject());
    assert_eq!(owner.0.lock().unwrap().references["example.service"].generation.as_deref(), Some("new"));
    drop(replacement);
    assert!(!owner.has_reference("example.service"));
}

#[tokio::test]
async fn a_published_candidate_reference_is_retained_until_ordinary_cleanup() {
    let (original, _original_server, _) = fixture(state()).await;
    let owner = Transport::new(original);
    let mut intent = owner.stage_reference("example.service", Some("ours".into()));
    intent.submitted();
    let (candidate, _server, state) = fixture(state()).await;
    assert!(owner.restore_candidate(&candidate, 0).await.unwrap());
    assert_eq!(state.refs.load(Ordering::SeqCst), 1);
    assert!(!intent.reject());
    assert!(owner.has_reference("example.service"));
}

//! Reference claims must settle independently without losing earlier ownership.

use super::*;

#[test]
fn abandoning_an_unsubmitted_intent_removes_only_its_reference() {
    let (connection, _server, _) = fixture(state());
    let owner = Transport::new(connection);
    owner.stage_reference("existing.service", "ours".into()).submitted();
    let intent = owner.stage_reference("rejected.service", "ours".into());
    drop(intent);
    assert!(!owner.has_reference("rejected.service"));
    assert!(owner.has_reference("existing.service"));
}

#[test]
fn rejecting_a_second_attempt_preserves_the_original_invocation() {
    let (connection, _server, _) = fixture(state());
    let owner = Transport::new(connection);
    owner.stage_reference("example.service", "ours".into()).submitted();
    owner.remember("example.service", &[1; 16]);
    let mut retry = owner.stage_reference("example.service", "other".into());
    retry.submitted();
    assert!(retry.reject());
    let state = owner.0.lock().unwrap();
    let reference = &state.references["example.service"];
    assert_eq!(reference.generation, "ours");
    assert_eq!(reference.invocation, Some(vec![1; 16]));
}

#[test]
fn overlapping_rejections_remove_the_intent_only_after_both_settle() {
    let (connection, _server, _) = fixture(state());
    let owner = Transport::new(connection);
    let mut first = owner.stage_reference("example.service", "ours".into());
    let mut second = owner.stage_reference("example.service", "ours".into());
    first.submitted();
    second.submitted();
    assert!(first.reject());
    assert!(owner.has_reference("example.service"));
    assert!(second.reject());
    assert!(!owner.has_reference("example.service"));
}

#[test]
fn an_accepted_overlap_survives_rejection_in_either_completion_order() {
    for accepted_first in [false, true] {
        let (connection, _server, _) = fixture(state());
        let owner = Transport::new(connection);
        let mut rejected = owner.stage_reference("example.service", "ours".into());
        let mut accepted = Some(owner.stage_reference("example.service", "ours".into()));
        rejected.submitted();
        accepted.as_mut().unwrap().submitted();
        if accepted_first { drop(accepted.take()); }
        assert!(rejected.reject());
        drop(accepted);
        assert!(owner.has_reference("example.service"));
        assert_eq!(owner.0.lock().unwrap().references["example.service"].pending, 0);
    }
}

#[test]
fn rejecting_an_old_intent_cannot_remove_a_replacement_claim() {
    let (connection, _server, _) = fixture(state());
    let owner = Transport::new(connection);
    let old = owner.stage_reference("example.service", "ours".into());
    owner.forget("example.service");
    let replacement = owner.stage_reference("example.service", "new".into());
    assert!(old.reject());
    assert_eq!(owner.0.lock().unwrap().references["example.service"].generation, "new");
    drop(replacement);
    assert!(!owner.has_reference("example.service"));
}

#[test]
fn a_published_candidate_reference_is_retained_until_ordinary_cleanup() {
    let (original, _original_server, _) = fixture(state());
    let owner = Transport::new(original);
    let mut intent = owner.stage_reference("example.service", "ours".into());
    intent.submitted();
    let (candidate, _server, state) = fixture(state());
    assert!(async_io::block_on(owner.restore_candidate(&candidate, 0)).unwrap());
    assert_eq!(state.refs.load(Ordering::SeqCst), 1);
    assert!(!intent.reject());
    assert!(owner.has_reference("example.service"));
}

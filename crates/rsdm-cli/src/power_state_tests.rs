use super::*;
use rsdm_session_manager::SessionStatus;

fn preparation(transaction: &str, result: &str) -> PreparationStatus {
    PreparationStatus { unit: "editor.service".into(), invocation_id: vec![1; 16], transaction_id: transaction.into(),
        method: "xsmp".into(), dispatch_state: "sent".into(), result: result.into(), escalated: false }
}

fn details(preparations: Vec<PreparationStatus>) -> SessionStatusDetails {
    SessionStatusDetails { enabled: true, state: "full".into(), status: SessionStatus {
        generation: "generation".into(), login_session_id: "test".into(), desktop_entry_id: "sway".into(),
        provider: "managed".into(), phase: "preparing".into(), xsmp_available: true,
        apps: vec![("editor.service".into(), "xsmp".into(), 30, "quitting".into())],
    }, capabilities: Vec::new(), limitations: Vec::new(), preparations }
}

#[test]
fn previous_preparations_do_not_count_as_current_progress() {
    let old = preparation("old", "clean");
    let mut progress = Progress::new();
    progress.initialize(details(vec![old.clone()]));
    assert!(!progress.apps["editor.service"].finished());
    progress.session(details(vec![old, preparation("current", "clean")]));
    assert!(progress.apps["editor.service"].finished());
    assert_eq!(progress.apps["editor.service"].method, "xsmp");
}

#[test]
fn disappearing_apps_and_unknown_results_are_never_reported_as_success() {
    let mut progress = Progress::new();
    progress.initialize(details(Vec::new()));
    let mut snapshot = details(vec![preparation("current", "unknown")]);
    snapshot.status.apps.clear();
    progress.session(snapshot);
    assert!(!progress.apps["editor.service"].finished());
    assert_eq!(progress.apps["editor.service"].state, "no longer tracked");
}

#[test]
fn forced_preparation_is_visible_and_keeps_the_original_wait() {
    let mut progress = Progress::new();
    progress.initialize(details(Vec::new()));
    let since = progress.apps["editor.service"].waiting;
    let mut forced = preparation("current", "forced");
    forced.escalated = true;
    progress.session(details(vec![forced]));
    let app = &progress.apps["editor.service"];
    assert!(app.finished());
    assert_eq!(app.method, "xsmp + forced termination");
    assert_eq!(app.waiting, since);
    assert!(app.ended.is_some());
    assert_eq!(app.methods_used, ["xsmp + forced termination"]);
}

#[test]
fn job_ids_and_departures_are_tracked_without_equating_departure_with_success() {
    let mut progress = Progress::new();
    let job = Job { id: 1, unit: "slow.service".into(), kind: "stop".into(), state: "running".into() };
    let status = LogindStatus { preparing: true, inhibitors: Vec::new() };
    progress.system(Ok(vec![job]), Ok(status.clone()));
    let since = progress.jobs[&1].since;
    progress.system(Ok(Vec::new()), Ok(status));
    assert!(!progress.jobs[&1].queued);
    assert!(progress.jobs[&1].ended.is_some());
    assert_eq!(progress.jobs[&1].since, since);
    assert_eq!(progress.stage(), 2);
    progress.system(Err("bus closed".into()), Err("bus closed".into()));
    assert_eq!(progress.system_errors.len(), 2);
    assert!(progress.observed_shutdown);
}

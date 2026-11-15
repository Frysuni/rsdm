use super::*;

#[test]
fn handoff_grace_ends_only_when_no_shutdown_has_been_observed() {
    let mut progress = Progress::new();
    assert!(monitor_finished(&progress, Instant::now()).is_none());
    let earlier = Instant::now() - Duration::from_secs(6);
    assert!(monitor_finished(&progress, earlier).unwrap().contains("no active system shutdown"));
    progress.system(Ok(Vec::new()), Ok(LogindStatus { preparing: true, inhibitors: Vec::new() }));
    assert!(monitor_finished(&progress, earlier).is_none());
}

#[test]
fn revocation_and_lost_telemetry_are_distinct_from_completed_shutdown() {
    let mut progress = Progress::new();
    progress.system(Ok(Vec::new()), Ok(LogindStatus { preparing: true, inhibitors: Vec::new() }));
    progress.system(Ok(Vec::new()), Ok(LogindStatus { preparing: false, inhibitors: Vec::new() }));
    assert!(monitor_finished(&progress, Instant::now()).unwrap().contains("may have been cancelled"));
    progress.system(Err("disconnected".into()), Err("disconnected".into()));
    assert!(monitor_finished(&progress, Instant::now()).unwrap().contains("status unavailable"));
}

#[test]
fn partial_logind_failure_keeps_pending_system_jobs_visible() {
    let mut progress = Progress::new();
    progress.system(Ok(vec![Job { id: 1, unit: "reboot.target".into(), kind: "start".into(), state: "waiting".into() }]),
        Err("logind stopped".into()));
    assert_eq!(progress.stage(), 2);
    assert!(monitor_finished(&progress, Instant::now()).is_none());
    assert!(progress.jobs[&1].queued);
}

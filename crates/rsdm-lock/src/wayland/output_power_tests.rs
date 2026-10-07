use std::sync::mpsc::{self, Receiver, SyncSender};

use super::*;

fn receive(receiver: &Receiver<(String, String)>) -> (String, String) {
    receiver.recv_timeout(Duration::from_secs(5)).unwrap()
}

fn paused_worker() -> (OutputPower, Receiver<(String, String)>, SyncSender<()>) {
    let (events, receiver) = mpsc::channel();
    let (release, wait) = mpsc::sync_channel(1);
    let worker = OutputPower::with_command(move |output, action, _| {
        events.send((output.into(), action.into())).unwrap();
        if action == "off" { wait.recv_timeout(Duration::from_secs(5)).unwrap(); }
        Ok(())
    }).unwrap();
    (worker, receiver, release)
}

#[test]
fn policy_change_remains_responsive_and_restores_a_late_off() {
    let (worker, events, release) = paused_worker();
    worker.power_off(vec!["DP-1".into(), "DP-2".into()]);
    assert_eq!(receive(&events), ("DP-1".into(), "off".into()));

    worker.restore();
    // The command is still paused; changing policy must not wait for it.
    assert_eq!(worker.requests.pending.lock().unwrap().outputs.len(), 0);
    release.send(()).unwrap();
    assert_eq!(receive(&events), ("DP-1".into(), "on".into()));
    drop(worker);
    assert!(events.try_recv().is_err(), "stale DP-2 Off must not execute");
}

#[test]
fn updates_coalesce_and_restore_precedes_the_replacement_topology() {
    let (worker, events, release) = paused_worker();
    worker.power_off(vec!["DP-1".into()]);
    assert_eq!(receive(&events), ("DP-1".into(), "off".into()));
    for number in 0..1_000 {
        worker.power_off(vec![format!("stale-{number}")]);
    }
    worker.restore();
    worker.power_off(vec!["DP-2".into()]);
    assert_eq!(worker.requests.pending.lock().unwrap().outputs, ["DP-2"]);
    release.send(()).unwrap();
    assert_eq!(receive(&events), ("DP-1".into(), "on".into()));
    assert_eq!(receive(&events), ("DP-2".into(), "off".into()));
    release.send(()).unwrap();
    drop(worker);
    assert_eq!(receive(&events), ("DP-2".into(), "on".into()));
    assert!(events.try_recv().is_err());
}

#[test]
fn removed_output_and_uncertain_off_keep_the_restoration_obligation() {
    let (events, receiver) = mpsc::channel();
    let worker = OutputPower::with_command(move |output, action, _| {
        events.send((output.into(), action.into())).unwrap();
        if action == "off" { return Err(io::ErrorKind::TimedOut.into()); }
        Ok(())
    }).unwrap();
    worker.power_off(vec!["DP-1".into()]);
    assert_eq!(receive(&receiver), ("DP-1".into(), "off".into()));
    worker.power_off(Vec::new());
    drop(worker);
    assert_eq!(receive(&receiver), ("DP-1".into(), "on".into()));
}

#[test]
fn failed_restore_is_retried_on_final_cleanup() {
    let (events, receiver) = mpsc::channel();
    let mut fail_restore = true;
    let worker = OutputPower::with_command(move |output, action, _| {
        events.send((output.into(), action.into())).unwrap();
        if action == "on" && std::mem::take(&mut fail_restore) {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    }).unwrap();
    worker.power_off(vec!["DP-1".into()]);
    assert_eq!(receive(&receiver), ("DP-1".into(), "off".into()));
    worker.restore();
    assert_eq!(receive(&receiver), ("DP-1".into(), "on".into()));
    drop(worker);
    assert_eq!(receive(&receiver), ("DP-1".into(), "on".into()));
}

#[test]
fn restoration_commands_share_one_deadline() {
    let mut owned = BTreeSet::from(["DP-1".into(), "DP-2".into(), "DP-3".into()]);
    let started = Instant::now();
    let mut deadlines = Vec::new();
    assert!(restore_outputs(&mut owned, &mut |_, _, deadline| {
        deadlines.push(deadline);
        Err(io::ErrorKind::TimedOut.into())
    }, &Requests::default()));
    assert_eq!(owned.len(), 3);
    assert_eq!(deadlines.len(), 3);
    for deadline in deadlines {
        assert!(deadline <= started + RESTORE_TIMEOUT);
        assert!(deadline <= Instant::now() + COMMAND_TIMEOUT);
    }
}

#[test]
fn missing_executable_does_not_claim_output_ownership() {
    let (events, receiver) = mpsc::channel();
    let worker = OutputPower::with_command(move |output, action, _| {
        events.send((output.into(), action.into())).unwrap();
        Err(io::ErrorKind::NotFound.into())
    }).unwrap();
    worker.power_off(vec!["DP-1".into()]);
    assert_eq!(receive(&receiver), ("DP-1".into(), "off".into()));
    drop(worker);
    assert!(receiver.try_recv().is_err());
}

#[test]
fn a_stopped_helper_times_out_before_the_queued_restore_runs() {
    let (events, receiver) = mpsc::channel();
    let worker = OutputPower::with_command(move |output, action, deadline| {
        events.send((output.into(), action.into())).unwrap();
        if action == "off" {
            let result = rsdm_infra::unix::run_command_until(
                Command::new("sh").args(["-c", "kill -STOP $$"]),
                deadline.min(Instant::now() + Duration::from_millis(200)),
            );
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    }).unwrap();
    worker.power_off(vec!["DP-1".into()]);
    assert_eq!(receive(&receiver), ("DP-1".into(), "off".into()));
    worker.restore();
    assert_eq!(receive(&receiver), ("DP-1".into(), "on".into()));
    drop(worker);
}

#[test]
fn final_cleanup_supersedes_an_inflight_restoration_pass() {
    let (events, receiver) = mpsc::channel();
    let (release, wait) = mpsc::sync_channel(1);
    let worker = OutputPower::with_command(move |output, action, _| {
        events.send((output.into(), action.into())).unwrap();
        if action == "on" { wait.recv_timeout(Duration::from_secs(5)).unwrap(); }
        Ok(())
    }).unwrap();
    worker.power_off(vec!["DP-1".into(), "DP-2".into(), "DP-3".into()]);
    for output in ["DP-1", "DP-2", "DP-3"] {
        assert_eq!(receive(&receiver), (output.into(), "off".into()));
    }
    worker.restore();
    assert_eq!(receive(&receiver), ("DP-1".into(), "on".into()));
    let requests = Arc::clone(&worker.requests);
    let cleanup = thread::spawn(move || drop(worker));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !requests.pending.lock().unwrap().shutdown {
        assert!(Instant::now() < deadline, "final cleanup must enqueue its request");
        thread::sleep(Duration::from_millis(1));
    }
    release.send(()).unwrap();
    assert_eq!(receive(&receiver), ("DP-2".into(), "on".into()));
    // The old pass must stop and consume Shutdown before issuing the next On.
    assert!(!requests.pending.lock().unwrap().shutdown);
    release.send(()).unwrap();
    assert_eq!(receive(&receiver), ("DP-3".into(), "on".into()));
    release.send(()).unwrap();
    cleanup.join().unwrap();
}

#[test]
fn coalescing_new_outputs_does_not_lose_an_interrupted_restore() {
    let (events, receiver) = mpsc::channel();
    let (release, wait) = mpsc::sync_channel(1);
    let worker = OutputPower::with_command(move |output, action, _| {
        events.send((output.into(), action.into())).unwrap();
        if (output, action) == ("DP-1", "on") {
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        Ok(())
    }).unwrap();
    worker.power_off(vec!["DP-1".into(), "DP-2".into()]);
    assert_eq!(receive(&receiver), ("DP-1".into(), "off".into()));
    assert_eq!(receive(&receiver), ("DP-2".into(), "off".into()));
    worker.restore();
    assert_eq!(receive(&receiver), ("DP-1".into(), "on".into()));
    worker.power_off(vec!["DP-3".into()]);
    release.send(()).unwrap();
    assert_eq!(receive(&receiver), ("DP-2".into(), "on".into()));
    assert_eq!(receive(&receiver), ("DP-3".into(), "off".into()));
    drop(worker);
    assert_eq!(receive(&receiver), ("DP-3".into(), "on".into()));
}

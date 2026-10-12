//! A captured scope keeps child processes owned after the original exits.

use std::{process::Stdio, sync::atomic::Ordering, time::Duration};

use tokio::io::{AsyncBufReadExt, BufReader};

use super::*;

async fn process_tree() -> (tokio::process::Child, u32, ProcessHandle) {
    let mut parent = tokio::process::Command::new("sh").args(["-c", "sleep 60 & echo $!; wait"])
        .stdout(Stdio::piped()).kill_on_drop(true).spawn().unwrap();
    let mut output = BufReader::new(parent.stdout.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), output.read_line(&mut line)).await.unwrap().unwrap();
    let pid = line.trim().parse().unwrap();
    let child = ProcessHandle::open(pid).unwrap().unwrap();
    (parent, pid, child)
}

#[tokio::test]
async fn scope_children_are_observed_and_signalled_after_the_original_process_exits() {
    let (fixture, mut app, supported) = migrated().await;
    if !supported { return; }
    let (mut parent, child_pid, child) = process_tree().await;
    let parent_pid = parent.id().unwrap();
    let original = ProcessHandle::open(parent_pid).unwrap().unwrap();
    app.original_process = Some(crate::app_origin::OriginalProcess { pid: parent_pid, pidfd_id: original.id().unwrap().unwrap() });
    fixture.state.main_pid.store(parent_pid, Ordering::SeqCst);
    fixture.state.scope_pid.store(parent_pid, Ordering::SeqCst);
    fixture.state.scope_foreign_pid.store(child_pid, Ordering::SeqCst);
    fixture.state.present.store(true, Ordering::SeqCst);
    *fixture.state.group.lock().unwrap() = "/unverified-service".into();
    let rejected = processes::terminate_main(&fixture.manager, &app, &crate::app_stop::ShutdownControl::default()).await;
    let original_alive = original.alive().unwrap();
    fixture.state.present.store(false, Ordering::SeqCst);
    let adopted = discover(&fixture.manager, &app).await.unwrap();
    parent.kill().await.unwrap();
    fixture.state.app_invocation.store(0, Ordering::SeqCst);
    fixture.state.scope_pid.store(child_pid, Ordering::SeqCst);
    fixture.state.scope_foreign_pid.store(0, Ordering::SeqCst);
    app.migrated_scope = adopted;
    let observed = processes::app_processes(&fixture.manager, &app).await;
    let signalled = processes::signal_app(&fixture.manager, &app, libc::SIGTERM, &crate::app_stop::ShutdownControl::default()).await;
    let exited = tokio::time::timeout(Duration::from_secs(2), async {
        while child.alive().unwrap() { tokio::time::sleep(Duration::from_millis(10)).await; }
    }).await;
    child.signal(libc::SIGKILL).unwrap();
    assert!(!rejected.unwrap());
    assert!(original_alive);
    assert!(app.migrated_scope.is_some());
    assert_eq!(observed.unwrap(), [child_pid]);
    assert!(signalled.unwrap());
    assert!(exited.is_ok());
    fixture.state.scope_present.store(false, Ordering::SeqCst);
    assert!(closed(&fixture.manager, &app).await.unwrap());
    assert!(processes::app_processes(&fixture.manager, &app).await.unwrap().is_empty());
}

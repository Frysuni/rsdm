//! Real signals are delivered only to a fresh test subprocess.

use std::time::Duration;

use super::*;

const CHILD_ENV: &str = "RSDM_SIGNAL_TEST_CHILD";
const TEST_NAME: &str = "signals::tests::termination_signals_wake_async_waits_in_a_fresh_process";

#[tokio::test]
async fn termination_signals_wake_async_waits_in_a_fresh_process() {
    if std::env::var_os(CHILD_ENV).is_some() {
        receive_termination_signals().await;
        return;
    }
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(CHILD_ENV, "1").kill_on_drop(true).spawn().unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait()).await.unwrap().unwrap();
    assert!(status.success());
}

async fn receive_termination_signals() {
    let mut signals = Signals::new().unwrap();
    let process = rsdm_system::process_handle::ProcessHandle::open(std::process::id()).unwrap().unwrap();
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let pending = signals.recv();
        tokio::pin!(pending);
        tokio::select! {
            result = &mut pending => panic!("a signal was reported before delivery: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {},
        }
        process.signal(signal).unwrap();
        tokio::time::timeout(Duration::from_secs(1), pending).await.unwrap().unwrap();
    }
}

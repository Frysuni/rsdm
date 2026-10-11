use tokio::io::{AsyncBufReadExt, BufReader};

use super::*;

#[tokio::test]
async fn detached_launcher_ownership_waits_for_actual_child_exit_and_preserves_status() {
    let provider = Provider { kind: crate::provider::ProviderKind::External, native_unit: None, logout_command: Vec::new() };
    let argv = ["/bin/sh".into(), "-c".into(), "while :; do :; done".into()];
    let mut process = SessionProcess::launch(&argv, &[], std::path::Path::new("/"), &provider, "unused.service".into()).unwrap();
    // Replace the launcher with a pipe-controlled child to prove its exit,
    // rather than waiting for a timer which could mask premature completion.
    process.child.as_ref().unwrap().lock().await.kill().await.unwrap();
    let mut child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "printf 'ready\\n'; read line; exit 7"])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut ready).await.unwrap();
    assert_eq!(ready, "ready\n");
    process.child = Some(Arc::new(tokio::sync::Mutex::new(child)));
    let observer = process.clone();
    drop(observer);
    let mut waiting = Box::pin(finish_child(&process));
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut waiting).await.is_err());
    tokio::io::AsyncWriteExt::write_all(&mut input, b"exit\n").await.unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(1), waiting).await.unwrap().unwrap(), 7);
}

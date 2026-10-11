//! Hot disable must interrupt identity verification before launching unmanaged.

use super::*;

struct PendingLogin(Arc<tokio::sync::Notify>);

#[zbus_macros::interface(name = "org.freedesktop.login1.Manager")]
impl PendingLogin {
    #[zbus(name = "GetSessionByPID")]
    async fn get_session_by_pid(&self, _pid: u32) -> OwnedObjectPath {
        self.0.notify_one();
        std::future::pending().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_during_a_stalled_login_query_launches_the_original_command_promptly() {
    let tools = SessionTools::new(false).await;
    let server = tools.connection.object_server();
    server.remove::<Login, _>("/org/freedesktop/login1").await.unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    server.at("/org/freedesktop/login1", PendingLogin(entered.clone())).await.unwrap();
    let mut command = tokio::process::Command::from(tools.command(&[
        "session", "start", "--", "/bin/sh", "-c", "printf done; exit 7",
    ]));
    let child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified()).await.unwrap();

    fs::write(tools.root.join("replacement.toml"), "[session_manager]\nenabled = false\n").unwrap();
    fs::rename(tools.root.join("replacement.toml"), tools.root.join("config.toml")).unwrap();
    let output = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait_with_output())
        .await.expect("disabled bootstrap must not wait for the five-second logind timeout").unwrap();
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    assert_eq!(output.stdout, b"done");
    assert!(!tools.root.join("rsdm").exists());
    assert!(tools.calls.lock().unwrap().is_empty());
}

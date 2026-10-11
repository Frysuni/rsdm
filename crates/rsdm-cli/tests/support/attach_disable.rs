use super::*;

struct PendingAttach(std::sync::Arc<tokio::sync::Notify>);

#[zbus_macros::interface(name = "org.rsdm.Session1")]
impl PendingAttach {
    async fn attach(&self) -> SessionStatus {
        self.0.notify_one();
        std::future::pending().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_during_attach_launches_the_app_once_and_leaves_finalize_passive() {
    for args in [
        vec!["app", "--", "/bin/sh", "-c", "printf done; exit 7"],
        vec!["session", "finalize"],
    ] {
        let tools = SessionTools::new(false).await;
        tools.connection.object_server().remove::<Session, _>("/org/rsdm/Session1").await.unwrap();
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        tools.connection.object_server().at("/org/rsdm/Session1", PendingAttach(entered.clone())).await.unwrap();
        let mut child = tokio::process::Command::from(tools.command(&args));
        child.stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let child = child.spawn().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified()).await.unwrap();
        fs::write(tools.root.join("config-next.toml"), "[session_manager]\nenabled = false\n").unwrap();
        fs::rename(tools.root.join("config-next.toml"), tools.root.join("config.toml")).unwrap();
        let output = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait_with_output()).await.unwrap().unwrap();
        if args[0] == "app" {
            assert_eq!(output.status.code(), Some(7));
            assert_eq!(output.stdout, b"done");
        } else {
            assert!(output.status.success(), "{output:?}");
            assert!(String::from_utf8_lossy(&output.stdout).contains("disabled"));
        }
        assert!(tools.calls.lock().unwrap().is_empty());
    }
}

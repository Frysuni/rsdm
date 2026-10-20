//! Status stays read-only when the user bus is absent or refuses connections.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_status_preserves_the_plain_application_list() {
    let tools = SessionTools::new(false).await;
    let output = tools.run(&["session", "status"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), format!(
        "managed: running (login test, desktop example, generation {GENERATION})\nRSDM XSMP: unavailable; auto uses the provider's shutdown method\nexample.service: registered, term, 30s\n"));
    assert!(output.stderr.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_or_refusing_user_bus_reports_an_inactive_coordinator() {
    let tools = SessionTools::new(false).await;
    let refused = tools.root.join("refused-bus");
    drop(std::os::unix::net::UnixListener::bind(&refused).unwrap());
    for socket in [tools.root.join("missing-bus"), refused] {
        let output = tools.command(&["session", "status"])
            .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", socket.display()))
            .output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "inactive: no session manager coordinator is running\n");
        assert!(output.stderr.is_empty());
    }
    assert!(tools.calls.lock().unwrap().is_empty());
    assert!(!tools.root.join("rsdm").exists());
}

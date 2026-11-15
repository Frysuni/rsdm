//! Exercise animation against private bus services, never the host's power manager.

use std::time::Duration;

use super::*;

pub(super) fn assert_restored(text: &str) {
    assert!(text.contains("\x1b[?1049h"), "alternate screen was not entered: {text}");
    let (_, summary) = text.rsplit_once("\x1b[?1049l").expect("alternate screen must be restored");
    assert!(text.contains("\x1b[?25h"), "cursor must be restored");
    assert_only_styles(summary);
}

struct SlowSession { calls: Calls, preparing: Arc<AtomicUsize> }

#[zbus_macros::interface(name = "org.rsdm.Session1")]
impl SlowSession {
    fn handshake(&self) -> &str { GENERATION }

    fn status(&self) -> SessionStatus {
        let preparing = self.preparing.load(Ordering::SeqCst) > 0;
        SessionStatus { generation: GENERATION.into(), login_session_id: "test".into(), desktop_entry_id: "sway".into(),
            provider: "managed".into(), phase: if preparing { "preparing" } else { "running" }.into(), xsmp_available: false,
            apps: vec![("slow-editor.service".into(), "term".into(), 30, if preparing { "quitting" } else { "registered" }.into())] }
    }

    async fn stop(&self, generation: &str, action: &str) -> StopOutcome {
        self.calls.lock().unwrap().push(format!("stop {generation} {action}"));
        self.preparing.store(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(2300)).await;
        StopOutcome { result: "cancelled".into(), forced_units: Vec::new(), message: "editor refused shutdown".into() }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_preparation_animates_and_shows_the_app_method_and_timeout() {
    let tools = SessionTools::new(false).await;
    tools.connection.object_server().remove::<Session, _>("/org/rsdm/Session1").await.unwrap();
    tools.connection.object_server().at("/org/rsdm/Session1", SlowSession {
        calls: tools.calls.clone(), preparing: Arc::new(AtomicUsize::new(0)),
    }).await.unwrap();
    let (status, text) = terminal(tools.command(&["power", "reboot"]), 100);
    assert_eq!(status.code(), Some(1));
    for expected in ["slow-editor.service", "quitting", "term", "timeout 30s", "◐", "◓", "editor refused shutdown", "POWER PROGRESS"] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert_restored(&text);
    assert_eq!(*tools.calls.lock().unwrap(), [format!("stop {GENERATION} reboot")]);
}

struct PowerLogin { calls: Calls, phase: Arc<AtomicUsize>, signal: Option<i32> }

#[zbus_macros::interface(name = "org.freedesktop.login1.Manager")]
impl PowerLogin {
    async fn reboot(&self, interactive: bool, #[zbus(connection)] connection: &zbus::Connection, #[zbus(header)] header: zbus::message::Header<'_>) {
        assert!(interactive);
        self.calls.lock().unwrap().push("logind reboot".into());
        self.phase.store(1, Ordering::SeqCst);
        let phase = self.phase.clone();
        let signal = self.signal;
        let pid = zbus::fdo::DBusProxy::new(connection).await.unwrap()
            .get_connection_unix_process_id(header.sender().unwrap().clone().into()).await.unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1400)).await;
            if let Some(signal) = signal {
                // SAFETY: pid came from the private bus credentials of this test's CLI.
                assert_eq!(unsafe { libc::kill(pid as i32, signal) }, 0);
            } else {
                phase.store(2, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(1400)).await;
                phase.store(3, Ordering::SeqCst);
            }
        });
    }

    fn list_inhibitors(&self) -> Vec<(String, String, String, String, u32, u32)> {
        if self.phase.load(Ordering::SeqCst) != 1 { return Vec::new(); }
        vec![("shutdown".into(), "Editor".into(), "Saving documents".into(), "delay".into(), 1000, 42)]
    }

    #[zbus(property)]
    fn preparing_for_shutdown(&self) -> bool { matches!(self.phase.load(Ordering::SeqCst), 1 | 2) }
}

struct SystemManager { phase: Arc<AtomicUsize> }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl SystemManager {
    fn list_jobs(&self) -> Vec<(u32, String, String, String, OwnedObjectPath, OwnedObjectPath)> {
        if !matches!(self.phase.load(Ordering::SeqCst), 1 | 2) { return Vec::new(); }
        vec![(1, "slow-backup.service".into(), "stop".into(), "running".into(),
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/1").unwrap(),
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/backup").unwrap())]
    }
}

async fn system_tools(signal: Option<i32>) -> SessionTools {
    let tools = SessionTools::new(false).await;
    fs::write(tools.root.join("config.toml"), "[session_manager]\nenabled = false\n").unwrap();
    tools.connection.object_server().remove::<Login, _>("/org/freedesktop/login1").await.unwrap();
    let phase = Arc::new(AtomicUsize::new(0));
    tools.connection.object_server().at("/org/freedesktop/login1", PowerLogin {
        calls: tools.calls.clone(), phase: phase.clone(), signal,
    }).await.unwrap();
    tools.connection.object_server().at("/org/freedesktop/systemd1", SystemManager { phase }).await.unwrap();
    tools.connection.request_name("org.freedesktop.systemd1").await.unwrap();
    tools
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_power_keeps_showing_system_jobs_and_inhibitor_reasons() {
    let tools = system_tools(None).await;
    let (status, text) = terminal(tools.command(&["power", "reboot"]), 100);
    assert!(status.success(), "{text}");
    for expected in ["Stage 3/3", "slow-backup.service", "Saving", "documents", "delay holder", "no longer reported as active"] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert_restored(&text);
    assert_eq!(*tools.calls.lock().unwrap(), ["logind reboot"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_and_termination_restore_the_terminal_without_replaying_power() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let tools = system_tools(Some(signal)).await;
        let (status, text) = terminal(tools.command(&["power", "reboot"]), 80);
        assert!(status.success(), "{text}");
        let reason = if signal == libc::SIGINT { "Stopped watching" } else { "termination signal" };
        assert!(text.contains("POWER MONITOR") && text.contains(reason), "{text}");
        assert_restored(&text);
        assert_eq!(*tools.calls.lock().unwrap(), ["logind reboot"]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_power_output_keeps_the_original_result_and_does_not_start_monitoring() {
    let tools = system_tools(None).await;
    let output = tools.run(&["power", "reboot"]);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"accepted: logind accepted reboot\n");
    assert!(output.stderr.is_empty());
    assert_eq!(*tools.calls.lock().unwrap(), ["logind reboot"]);
}

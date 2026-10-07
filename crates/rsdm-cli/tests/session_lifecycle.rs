//! Real CLI requests on an isolated bus; never connect to the running desktop.

use std::{
    fs, io::{BufRead, BufReader}, os::unix::fs::PermissionsExt, path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}},
};

use rsdm_infra::session_manager::{SessionStatus, StopOutcome};
use zbus::zvariant::OwnedObjectPath;

#[path = "support/terminal.rs"]
mod terminal_output;
use terminal_output::{assert_only_styles, terminal};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
const GENERATION: &str = "0123456789abcdef0123456789abcdef";
type Calls = Arc<Mutex<Vec<String>>>;
type Launch = (String, Vec<String>, Vec<(String, String)>, String, u64, String, String, Vec<String>);

struct Login;

#[zbus_macros::interface(name = "org.freedesktop.login1.Manager")]
impl Login {
    fn get_session(&self, _id: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/org/freedesktop/login1/session/test").unwrap()
    }
}

struct LoginSession;

#[zbus_macros::interface(name = "org.freedesktop.login1.Session")]
impl LoginSession {
    #[zbus(property)]
    fn user(&self) -> (u32, OwnedObjectPath) {
        // SAFETY: geteuid has no preconditions.
        (unsafe { libc::geteuid() }, OwnedObjectPath::try_from("/org/freedesktop/login1/user/test").unwrap())
    }

    #[zbus(property)]
    fn id(&self) -> &str { "test" }
}

struct Session { calls: Calls, finalize_fails: bool }

#[zbus_macros::interface(name = "org.rsdm.Session1")]
impl Session {
    fn finalize(&self, generation: &str, _environment: Vec<(String, String)>) -> zbus::fdo::Result<()> {
        self.calls.lock().unwrap().push(format!("finalize {generation}"));
        if self.finalize_fails { return Err(zbus::fdo::Error::Failed("session is shutting down".into())); }
        Ok(())
    }

    fn launch(&self, request: Launch) -> String {
        self.calls.lock().unwrap().push(format!("launch {:?} {} {} {:?}", request.1, request.4, request.5, request.7));
        "example.service".into()
    }

    fn cancel(&self, generation: &str) {
        self.calls.lock().unwrap().push(format!("cancel {generation}"));
    }

    fn status(&self) -> SessionStatus {
        SessionStatus { generation: GENERATION.into(), login_session_id: "test".into(), desktop_entry_id: "example".into(),
            provider: "managed".into(), phase: "running".into(), xsmp_available: false,
            apps: vec![("example.service".into(), "term".into(), 30, "registered".into())] }
    }

    fn stop(&self, generation: &str, action: &str) -> StopOutcome {
        self.calls.lock().unwrap().push(format!("stop {generation} {action}"));
        StopOutcome { result: "cancelled".into(), forced_units: vec!["earlier.service".into()], message: "application timed out".into() }
    }
}

struct SessionTools {
    root: PathBuf,
    bus: Child,
    connection: zbus::blocking::Connection,
    address: String,
    calls: Calls,
}

impl SessionTools {
    fn new(finalize_fails: bool) -> Self {
        let root = std::env::temp_dir().join(format!("rsdm-session-test-{}-{}", std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(root.join("config.toml"), "").unwrap();
        let mut bus = Command::new("dbus-daemon").args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("tests require dbus-daemon");
        let mut address = String::new();
        BufReader::new(bus.stdout.take().unwrap()).read_line(&mut address).unwrap();
        let address = address.trim().to_string();
        let calls = Calls::default();
        let connection = zbus::blocking::connection::Builder::address(address.as_str()).unwrap()
            .serve_at("/org/freedesktop/login1", Login).unwrap()
            .serve_at("/org/freedesktop/login1/session/test", LoginSession).unwrap()
            .serve_at("/org/rsdm/Session1", Session { calls: calls.clone(), finalize_fails }).unwrap()
            .name("org.freedesktop.login1").unwrap().name("org.rsdm.Session1").unwrap().build().unwrap();
        Self { root, bus, connection, address, calls }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rsdm"));
        command.env_clear()
            .env("PATH", &self.root).env("HOME", &self.root).env("XDG_RUNTIME_DIR", &self.root)
            .env("TERM", "xterm-256color")
            .env("DBUS_SESSION_BUS_ADDRESS", &self.address).env("DBUS_SYSTEM_BUS_ADDRESS", &self.address)
            .env("XDG_SESSION_ID", "test").env("RSDM_SESSION_GENERATION", GENERATION)
            .arg("--config").arg(self.root.join("config.toml")).args(args);
        command
    }
}

impl Drop for SessionTools {
    fn drop(&mut self) {
        let _ = &self.connection;
        let _ = self.bus.kill();
        let _ = self.bus.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn starting_a_second_session_does_not_stop_or_modify_the_first() {
    let tools = SessionTools::new(false);
    let output = tools.run(&["session", "start", "--", "compositor"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("name already taken"), "{:?}", output);
    assert!(tools.calls.lock().unwrap().is_empty());
}

#[test]
fn failed_finalize_returns_the_coordinator_failure_to_the_caller() {
    let tools = SessionTools::new(true);
    let output = tools.run(&["session", "finalize"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("session is shutting down"));
    assert_eq!(*tools.calls.lock().unwrap(), [format!("finalize {GENERATION}")]);
}

#[test]
fn finalize_is_routed_to_the_current_coordinator() {
    let tools = SessionTools::new(false);
    assert!(tools.run(&["session", "finalize"]).status.success());
    assert_eq!(*tools.calls.lock().unwrap(), [format!("finalize {GENERATION}")]);
}

#[test]
fn app_options_preserve_literal_arguments_and_parse_quit_argv_without_a_shell() {
    let tools = SessionTools::new(false);
    let output = tools.run(&["app", "--shutdown-timeout", "8", "--on-timeout", "cancel",
        "--quit-command", "examplectl \"quit now\"", "--", "example", "$HOME", "", "--on-timeout"]);
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stdout.is_empty(), "app launch must remain quiet outside a terminal");
    assert_eq!(*tools.calls.lock().unwrap(), ["launch [\"example\", \"$HOME\", \"\", \"--on-timeout\"] 8 cancel [\"examplectl\", \"quit now\"]"]);
}

#[test]
fn cancelled_logout_is_not_reported_as_success() {
    let tools = SessionTools::new(false);
    let output = tools.run(&["session", "stop"]);
    assert!(!output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "cancelled: application timed out\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("forced shutdown: earlier.service"));
    assert!(!output.stdout.contains(&0x1b) && !output.stderr.contains(&0x1b));
}

#[test]
fn session_status_preserves_the_plain_application_list() {
    let tools = SessionTools::new(false);
    let output = tools.run(&["session", "status"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), format!(
        "managed: running (login test, desktop example, generation {GENERATION})\nRSDM XSMP: unavailable; auto uses the provider's shutdown method\nexample.service: registered, term, 30s\n"));
    assert!(output.stderr.is_empty());
}

#[test]
fn session_confirmations_and_registered_apps_are_styled_in_a_terminal() {
    let tools = SessionTools::new(false);
    for (args, title) in [
        (&["session", "finalize"][..], "ENVIRONMENT PUBLISHED"),
        (&["session", "cancel"][..], "CANCELLATION REQUESTED"),
        (&["app", "--", "example"][..], "APPLICATION LAUNCHED"),
        (&["session", "status"][..], "SESSION STATUS"),
    ] {
        let (status, text) = terminal(tools.command(args), 80);
        assert!(status.success(), "{text}");
        assert!(text.contains(title) && text.contains('╭') && text.contains('\x1b'), "{text}");
        if args.last() == Some(&"status") {
            assert!(text.contains("APPLICATIONS") && text.contains("example.service") && text.contains("registered"), "{text}");
        }
        assert_only_styles(&text);
    }
    assert!(tools.run(&["session", "cancel"]).stdout.is_empty());
    assert!(tools.calls.lock().unwrap().contains(&format!("cancel {GENERATION}")));
}

#[test]
fn cancelled_session_and_power_requests_keep_failure_status_in_a_terminal() {
    let tools = SessionTools::new(false);
    for (args, action) in [
        (&["session", "stop"][..], "logout"),
        (&["power", "reboot"][..], "reboot"),
        (&["power", "poweroff"][..], "poweroff"),
    ] {
        let (status, text) = terminal(tools.command(args), 80);
        assert_eq!(status.code(), Some(1), "{text}");
        for expected in ["SESSION REQUEST", "cancelled", "FORCED SHUTDOWN", "earlier.service", "COMMAND FAILED"] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert_only_styles(&text);
        assert!(tools.calls.lock().unwrap().contains(&format!("stop {GENERATION} {action}")));
    }
}

#[test]
fn expired_stop_hook_and_recovery_budgets_skip_the_bus_handshake() {
    let tools = SessionTools::new(false);
    let session = tools.root.join("rsdm/sessions").join(GENERATION);
    for path in [tools.root.join("rsdm"), tools.root.join("rsdm/sessions"), session.clone(), session.join("apps")] {
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let record = session.join("session.toml");
    let contents = format!("anchor_unit = 'rsdm-session-{GENERATION}.service'\n\
        compositor_invocation = []\nprovider = 'managed'\nowns_targets = false\n\
        phase = 'preparing'\nshutdown_deadline_usec = 1\n\
        [identity]\ngeneration = '{GENERATION}'\nlogin_session_id = 'test'\nuid = {uid}\n");
    fs::write(&record, &contents).unwrap();
    fs::set_permissions(&record, fs::Permissions::from_mode(0o600)).unwrap();
    let socket = tools.root.join("silent-bus");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = format!("unix:path={}", socket.display());
    let unit = format!("app-rsdm-test@{GENERATION}-1.service");

    for args in [
        vec!["session", "app-stop", "--generation", GENERATION, "--unit", unit.as_str()],
        vec!["session", "cleanup", "--generation", GENERATION],
    ] {
        let before = std::time::Instant::now();
        let output = tools.command(&args).env("DBUS_SESSION_BUS_ADDRESS", &address).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("session shutdown deadline expired"), "{output:?}");
        assert!(before.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(fs::read_to_string(&record).unwrap(), contents);
    }
}

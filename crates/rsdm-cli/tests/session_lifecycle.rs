//! Real CLI requests on an isolated bus; never connect to the running desktop.

use std::{
    fs, io::{BufRead, BufReader}, os::unix::fs::PermissionsExt, path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}},
};

use rsdm_infra::session_manager::{SessionStatus, StopOutcome};
use zbus::zvariant::OwnedObjectPath;

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
        Command::new(env!("CARGO_BIN_EXE_rsdm")).env_clear()
            .env("PATH", &self.root).env("HOME", &self.root).env("XDG_RUNTIME_DIR", &self.root)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.address).env("DBUS_SYSTEM_BUS_ADDRESS", &self.address)
            .env("XDG_SESSION_ID", "test").env("RSDM_SESSION_GENERATION", GENERATION)
            .arg("--config").arg(self.root.join("config.toml")).args(args).output().unwrap()
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

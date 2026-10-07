use std::{os::unix::net::UnixStream, sync::{Arc, Mutex}, time::Instant};

use super::*;

type Calls = Arc<Mutex<Vec<String>>>;

struct Logind {
    answer: &'static str,
    hung: bool,
    lost_reply: bool,
    calls: Calls,
}

impl Logind {
    async fn check(&self, name: &str) -> String {
        self.calls.lock().unwrap().push(name.into());
        if self.hung { std::future::pending::<()>().await; }
        self.answer.into()
    }

    async fn request(&self, name: &str, interactive: bool) -> zbus::fdo::Result<()> {
        assert!(interactive);
        self.calls.lock().unwrap().push(name.into());
        if self.hung { std::future::pending::<()>().await; }
        if self.lost_reply { return Err(zbus::fdo::Error::NoReply("reply lost after accepting power request".into())); }
        Ok(())
    }
}

#[zbus_macros::interface(name = "org.freedesktop.login1.Manager")]
impl Logind {
    async fn can_reboot(&self) -> String { self.check("CanReboot").await }
    async fn can_power_off(&self) -> String { self.check("CanPowerOff").await }
    async fn reboot(&self, interactive: bool) -> zbus::fdo::Result<()> { self.request("Reboot", interactive).await }
    async fn power_off(&self, interactive: bool) -> zbus::fdo::Result<()> { self.request("PowerOff", interactive).await }
    async fn suspend(&self, interactive: bool) -> zbus::fdo::Result<()> { self.request("Suspend", interactive).await }
    async fn hibernate(&self, interactive: bool) -> zbus::fdo::Result<()> { self.request("Hibernate", interactive).await }
}

fn fixture(answer: &'static str, hung: bool, lost_reply: bool) -> (Connection, Connection, Calls) {
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let calls = Calls::default();
    let state = calls.clone();
    let server = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server_socket).p2p().server(zbus::Guid::generate()).unwrap()
            .serve_at(PATH, Logind { answer, hung, lost_reply, calls: state }).unwrap().build().await.unwrap()
    }));
    let client = async_io::block_on(zbus::connection::Builder::unix_stream(client_socket).p2p().build()).unwrap();
    (client, server.join().unwrap(), calls)
}

fn assert_timed_out(result: Result<(), SessionError>, before: Instant) {
    assert!(matches!(result, Err(SessionError::Bus(error)) if error.to_string().contains("logind request timed out")));
    assert!(before.elapsed() < Duration::from_secs(1));
}

#[test]
fn power_authorization_keeps_yes_challenge_and_denial_semantics() {
    for action in ["reboot", "poweroff"] {
        for answer in ["yes", "challenge", "no", "na", "unexpected"] {
            let (connection, _server, calls) = fixture(answer, false, false);
            let result = async_io::block_on(check_with(async { Ok(connection) }, action, Duration::from_secs(1)));
            assert_eq!(result.is_ok(), matches!(answer, "yes" | "challenge"));
            assert_eq!(*calls.lock().unwrap(), [format!("Can{}", method(action).unwrap())]);
        }
    }
}

#[test]
fn direct_power_requests_are_interactive_and_sent_once() {
    for action in ["reboot", "poweroff", "suspend", "hibernate"] {
        let (connection, _server, calls) = fixture("yes", false, false);
        async_io::block_on(request_with(async { Ok(connection) }, action, Duration::from_secs(1))).unwrap();
        assert_eq!(*calls.lock().unwrap(), [method(action).unwrap()]);
    }
}

#[test]
fn lost_power_replies_are_never_replayed() {
    let (connection, _server, calls) = fixture("yes", false, true);
    let result = async_io::block_on(request_with(async { Ok(connection) }, "reboot", Duration::from_secs(1)));
    assert!(result.is_err());
    assert_eq!(*calls.lock().unwrap(), ["Reboot"]);
}

#[test]
fn power_timeouts_include_connection_setup() {
    let before = Instant::now();
    assert_timed_out(async_io::block_on(check_with(std::future::pending(), "reboot", Duration::from_millis(50))), before);
    let before = Instant::now();
    assert_timed_out(async_io::block_on(request_with(std::future::pending(), "poweroff", Duration::from_millis(50))), before);
}

#[test]
fn hung_authorization_and_power_replies_share_the_call_budget() {
    let (connection, _server, calls) = fixture("yes", true, false);
    let before = Instant::now();
    assert_timed_out(async_io::block_on(check_with(async { Ok(connection) }, "reboot", Duration::from_millis(50))), before);
    assert_eq!(*calls.lock().unwrap(), ["CanReboot"]);
    let (connection, _server, calls) = fixture("yes", true, false);
    let before = Instant::now();
    assert_timed_out(async_io::block_on(request_with(async { Ok(connection) }, "poweroff", Duration::from_millis(50))), before);
    assert_eq!(*calls.lock().unwrap(), ["PowerOff"]);
}

#[test]
fn unsupported_actions_fail_before_polling_any_connection() {
    let result = async_io::block_on(check_with(std::future::pending(), "logout", Duration::from_secs(1)));
    assert!(matches!(result, Err(SessionError::State(message)) if message == "unsupported power action"));
    let result = async_io::block_on(request_with(std::future::pending(), "logout", Duration::from_secs(1)));
    assert!(matches!(result, Err(SessionError::State(message)) if message == "unsupported power action"));
}

//! Login identity queries against a private logind substitute.

use std::{sync::{Arc, Mutex}, time::Instant};

use super::*;

const SESSION_PATH: &str = "/org/freedesktop/login1/session/test";

struct Manager { calls: Arc<Mutex<Vec<String>>>, missing: bool }

#[zbus_macros::interface(name = "org.freedesktop.login1.Manager")]
impl Manager {
    fn get_session(&self, id: &str) -> OwnedObjectPath {
        self.calls.lock().unwrap().push(format!("session:{id}"));
        OwnedObjectPath::try_from(SESSION_PATH).unwrap()
    }

    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, pid: u32) -> Result<OwnedObjectPath, LoginError> {
        self.calls.lock().unwrap().push(format!("pid:{pid}"));
        if self.missing { return Err(LoginError::NoSessionForPID("user service has no login cgroup".into())); }
        Ok(OwnedObjectPath::try_from(SESSION_PATH).unwrap())
    }

    fn get_user(&self, uid: u32) -> OwnedObjectPath {
        self.calls.lock().unwrap().push(format!("user:{uid}"));
        OwnedObjectPath::try_from("/org/freedesktop/login1/user/test").unwrap()
    }
}

#[derive(Debug, zbus_macros::DBusError)]
#[zbus(prefix = "org.freedesktop.login1")]
enum LoginError {
    NoSessionForPID(String),
    #[zbus(error)]
    Bus(zbus::Error),
}

struct User(String);

#[zbus_macros::interface(name = "org.freedesktop.login1.User")]
impl User {
    #[zbus(property)]
    fn display(&self) -> (String, OwnedObjectPath) {
        (self.0.clone(), OwnedObjectPath::try_from(SESSION_PATH).unwrap())
    }
}

struct Session(u32);

#[zbus_macros::interface(name = "org.freedesktop.login1.Session")]
impl Session {
    #[zbus(property)]
    fn user(&self) -> (u32, OwnedObjectPath) {
        (self.0, OwnedObjectPath::try_from("/org/freedesktop/login1/user/test").unwrap())
    }

    #[zbus(property)]
    fn id(&self) -> &str { "verified-session" }
}

async fn fixture(uid: u32) -> (zbus::Connection, zbus::Connection, Arc<Mutex<Vec<String>>>) {
    fixture_with_display(uid, None).await
}

async fn fixture_with_display(uid: u32, display: Option<&str>) -> (zbus::Connection, zbus::Connection, Arc<Mutex<Vec<String>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let manager = Manager { calls: calls.clone(), missing: display.is_some() };
    let user = User(display.unwrap_or_default().to_string());
    let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket).p2p()
            .server(zbus::Guid::generate()).unwrap()
            .serve_at("/org/freedesktop/login1", manager).unwrap()
            .serve_at("/org/freedesktop/login1/user/test", user).unwrap()
            .serve_at(SESSION_PATH, Session(uid)).unwrap().build().await.unwrap()
    });
    let client = zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap();
    (client, server.await.unwrap(), calls)
}

#[tokio::test]
async fn a_shared_user_service_discovers_the_graphical_login_without_a_hint() {
    let (connection, _server, calls) = fixture_with_display(1000, Some("verified-session")).await;
    let id = verified_login_session(async { Ok(connection) }, 1000, None).await.unwrap();
    assert_eq!(id, "verified-session");
    assert_eq!(*calls.lock().unwrap(), [format!("pid:{}", std::process::id()), "user:1000".into()]);
}

#[tokio::test]
async fn shared_user_services_require_a_display_with_the_correct_owner_and_hint() {
    for (owner, display, hint) in [
        (1000, "", None),
        (1001, "verified-session", None),
        (1000, "verified-session", Some("other-session")),
    ] {
        let (connection, _server, _) = fixture_with_display(owner, Some(display)).await;
        assert!(verified_login_session(async { Ok(connection) }, 1000, hint).await.is_err());
    }
}

#[tokio::test]
async fn a_matching_session_hint_is_verified_against_the_current_process() {
    let (connection, _server, calls) = fixture(1000).await;
    let id = verified_login_session(async { Ok(connection) }, 1000, Some("verified-session")).await.unwrap();
    assert_eq!(id, "verified-session");
    assert_eq!(*calls.lock().unwrap(), [format!("pid:{}", std::process::id())]);
}

#[tokio::test]
async fn another_session_of_the_same_user_cannot_be_selected_by_an_environment_hint() {
    let (connection, _server, calls) = fixture(1000).await;
    let result = verified_login_session(async { Ok(connection) }, 1000, Some("other-session")).await;
    assert!(matches!(result, Err(SessionError::State(error)) if error == "logind session hint does not match the current process"));
    assert_eq!(*calls.lock().unwrap(), [format!("pid:{}", std::process::id())]);
}

#[tokio::test]
async fn no_hint_queries_the_current_process_instead() {
    let (connection, _server, calls) = fixture(1000).await;
    let id = verified_login_session(async { Ok(connection) }, 1000, None).await.unwrap();
    assert_eq!(id, "verified-session");
    assert_eq!(*calls.lock().unwrap(), [format!("pid:{}", std::process::id())]);
}

#[tokio::test]
async fn caller_verification_queries_the_supplied_process_instead_of_the_coordinator() {
    let (connection, _server, calls) = fixture(1000).await;
    let manager = zbus::Proxy::new(&connection, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").await.unwrap();
    assert_eq!(login_session_for_process(&manager, 12345, 1000).await.unwrap(), "verified-session");
    assert_eq!(*calls.lock().unwrap(), ["pid:12345"]);
}

#[tokio::test]
async fn a_session_owned_by_another_uid_is_rejected() {
    let (connection, _server, _) = fixture(1001).await;
    let result = verified_login_session(async { Ok(connection) }, 1000, Some("hint")).await;
    assert!(matches!(result, Err(SessionError::State(error)) if error == "logind session belongs to another user"));
}

#[tokio::test]
async fn the_budget_also_bounds_a_hung_connection_setup() {
    let started = Instant::now();
    let result = verified_login_session(std::future::pending(), 1000, None).await;
    assert!(matches!(result, Err(SessionError::Bus(error)) if super::super::bus::retryable_error(&error)));
    assert!(started.elapsed() < Duration::from_secs(6));
}

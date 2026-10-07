use std::{future, os::unix::net::UnixStream, time::Instant};

use super::*;

enum Answer {
    Text(&'static str),
    Error,
    Hung,
}

struct Logind {
    hibernate: Answer,
    suspend: Answer,
}

#[zbus_macros::interface(name = "org.freedesktop.login1.Manager")]
impl Logind {
    async fn can_hibernate(&self) -> zbus::fdo::Result<String> { answer(&self.hibernate).await }
    async fn can_suspend(&self) -> zbus::fdo::Result<String> { answer(&self.suspend).await }
}

async fn answer(answer: &Answer) -> zbus::fdo::Result<String> {
    match answer {
        Answer::Text(value) => Ok((*value).into()),
        Answer::Error => Err(zbus::fdo::Error::AccessDenied("capability denied".into())),
        Answer::Hung => future::pending().await,
    }
}

fn connect(hibernate: Answer, suspend: Answer) -> (Connection, Connection) {
    let (server, client) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server).p2p()
            .server(zbus::Guid::generate()).unwrap()
            .serve_at(PATH, Logind { hibernate, suspend }).unwrap().build().await.unwrap()
    }));
    let client = async_io::block_on(async {
        zbus::connection::Builder::unix_stream(client).p2p().build().await.unwrap()
    });
    (client, worker.join().unwrap())
}

#[test]
fn only_authorized_or_challenge_capabilities_are_offered() {
    for (hibernate, suspend, expected) in [
        ("yes", "challenge", PowerCapabilities { hibernate: true, suspend: true }),
        ("challenge", "no", PowerCapabilities { hibernate: true, suspend: false }),
        ("na", "yes", PowerCapabilities { hibernate: false, suspend: true }),
        ("no", "unexpected", PowerCapabilities::default()),
    ] {
        let (connection, _server) = connect(Answer::Text(hibernate), Answer::Text(suspend));
        let actual = async_io::block_on(query_capabilities(
            future::ready(Ok(connection)), Duration::from_secs(2),
        )).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn one_denied_method_does_not_hide_another_available_action() {
    let (connection, _server) = connect(Answer::Error, Answer::Text("yes"));
    let actual = async_io::block_on(query_capabilities(
        future::ready(Ok(connection)), Duration::from_secs(2),
    )).unwrap();
    assert_eq!(actual, PowerCapabilities { hibernate: false, suspend: true });
}

#[test]
fn capability_deadline_includes_bus_connection_setup() {
    let started = Instant::now();
    let result = async_io::block_on(query_capabilities(future::pending(), Duration::from_millis(50)));
    assert!(matches!(result, Err(SessionError::State(message)) if message.contains("timed out")));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_hung_logind_method_is_bounded_by_the_query_deadline() {
    let (connection, _server) = connect(Answer::Hung, Answer::Text("yes"));
    let started = Instant::now();
    let result = async_io::block_on(query_capabilities(
        future::ready(Ok(connection)), Duration::from_millis(100),
    ));
    assert!(matches!(result, Err(SessionError::State(message)) if message.contains("timed out")));
    assert!(started.elapsed() < Duration::from_secs(1));
}

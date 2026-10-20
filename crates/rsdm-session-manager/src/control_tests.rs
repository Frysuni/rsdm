use std::{time::Duration};

use super::*;

async fn connect(uid: u32) -> (Connection, Connection, tokio::sync::mpsc::Receiver<Request>) {
    let (endpoint, receiver) = Endpoint::for_test(super::super::identity::SessionIdentity {
        generation: "0123456789abcdef0123456789abcdef".into(), login_session_id: "test".into(),
        desktop_entry_id: None, uid,
    }).await;
    connect_endpoint(endpoint, receiver).await
}

async fn connect_endpoint(endpoint: Endpoint, receiver: tokio::sync::mpsc::Receiver<Request>) -> (Connection, Connection, tokio::sync::mpsc::Receiver<Request>) {
    let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket.into_std().unwrap())
            .p2p().server(zbus::Guid::generate())
            .serve_at(OBJECT_PATH, endpoint).build().await.unwrap()
    });
    let client = zbus::connection::Builder::unix_stream(client_socket.into_std().unwrap())
        .p2p().build().await.unwrap();
    (server.await.unwrap(), client, receiver)
}

#[path = "control_authorization_tests.rs"]
mod authorization;

fn launch() -> LaunchRequest {
    LaunchRequest {
        generation: "0123456789abcdef0123456789abcdef".into(),
        argv: vec!["example".into()],
        environment: Vec::new(),
        directory: "/".into(),
        timeout_secs: 30,
        on_timeout: "force".into(),
        method: "auto".into(),
        quit_command: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pending_launch_does_not_block_a_cancel_request() {
    // SAFETY: geteuid has no preconditions.
    let (_server, connection, mut requests) = connect(unsafe { libc::geteuid() }).await;
    let launching = connection.clone();
    let worker = tokio::spawn(async move {
        proxy(&launching).await.unwrap().call::<_, _, String>("Launch", &(launch(),)).await.unwrap()
    });
    let Request::Launch(_, pending) = tokio::time::timeout(Duration::from_secs(2), requests.recv()).await.unwrap().unwrap() else { panic!("expected launch"); };
    let cancelling = connection.clone();
    let cancel = tokio::spawn(async move {
        proxy(&cancelling).await.unwrap().call::<_, _, ()>("Cancel", &(launch().generation,)).await.unwrap()
    });
    let Request::Cancel { reply, .. } = tokio::time::timeout(Duration::from_secs(2), requests.recv()).await.unwrap().unwrap() else { panic!("expected cancel"); };
    reply.try_send(Ok(())).unwrap();
    cancel.await.unwrap();
    pending.try_send(Ok("example.service".into())).unwrap();
    assert_eq!(worker.await.unwrap(), "example.service");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_uid_cannot_submit_session_requests() {
    // SAFETY: geteuid has no preconditions.
    let other_uid = unsafe { libc::geteuid() }.wrapping_add(1);
    let (_server, connection, mut requests) = connect(other_uid).await;
    let result = proxy(&connection).await.unwrap().call::<_, _, SessionStatus>("Status", &()).await;
    assert!(result.unwrap_err().to_string().contains("another user"));
    assert!(requests.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_completion_is_dispatched_before_the_coordinator_can_exit() {
    // SAFETY: geteuid has no preconditions.
    let (_server, connection, mut requests) = connect(unsafe { libc::geteuid() }).await;
    let worker = tokio::spawn(async move {
        proxy(&connection).await.unwrap().call::<_, _, StopOutcome>("Stop", &(launch().generation, "logout")).await.unwrap()
    });
    let Request::Stop { reply, .. } = tokio::time::timeout(Duration::from_secs(2), requests.recv()).await.unwrap().unwrap() else { panic!("expected stop"); };
    reply.try_send(Ok(StopOutcome { result: "completed".into(), forced_units: Vec::new(), message: String::new() })).unwrap();
    assert!(matches!(tokio::time::timeout(Duration::from_secs(2), requests.recv()).await.unwrap().unwrap(), Request::StopReplySent(_)));
    assert_eq!(worker.await.unwrap().result, "completed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_payloads_do_not_reach_the_coordinator() {
    // SAFETY: geteuid has no preconditions.
    let (_server, connection, mut requests) = connect(unsafe { libc::geteuid() }).await;
    let mut request = launch();
    request.argv.push("x".repeat(64 * 1024 + 1));
    let error = proxy(&connection).await.unwrap().call::<_, _, String>("Launch", &(request,)).await.unwrap_err();
    assert!(error.to_string().contains("InvalidArgs"));
    assert!(requests.try_recv().is_err());

    let environment = vec![("NAME".to_string(), "x".repeat(64 * 1024 + 1))];
    let error = proxy(&connection).await.unwrap().call::<_, _, ()>("Finalize", &(launch().generation, environment)).await.unwrap_err();
    assert!(error.to_string().contains("InvalidArgs"));
    assert!(requests.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_saturation_returns_busy_but_still_admits_cancellation() {
    // SAFETY: geteuid has no preconditions.
    let (_server, connection, mut requests) = connect(unsafe { libc::geteuid() }).await;
    let mut pending = Vec::new();
    for _ in 0..admission::MAX_REGULAR_REQUESTS {
        let launching = connection.clone();
        let worker = tokio::spawn(async move {
            proxy(&launching).await.unwrap().call::<_, _, String>("Launch", &(launch(),)).await.unwrap()
        });
        let Request::Launch(_, reply) = tokio::time::timeout(Duration::from_secs(2), requests.recv()).await.unwrap().unwrap() else {
            panic!("expected launch");
        };
        pending.push((reply, worker));
    }
    let error = proxy(&connection).await.unwrap().call::<_, _, String>("Launch", &(launch(),)).await.unwrap_err();
    assert!(error.to_string().contains("LimitsExceeded"));
    assert!(requests.try_recv().is_err());

    let cancelling = connection.clone();
    let cancel = tokio::spawn(async move {
        proxy(&cancelling).await.unwrap().call::<_, _, ()>("Cancel", &(launch().generation,)).await.unwrap()
    });
    let Request::Cancel { reply, .. } = tokio::time::timeout(Duration::from_secs(2), requests.recv()).await.unwrap().unwrap() else {
        panic!("expected reserved cancellation");
    };
    reply.try_send(Ok(())).unwrap();
    cancel.await.unwrap();
    for (reply, worker) in pending {
        reply.try_send(Ok("example.service".into())).unwrap();
        assert_eq!(worker.await.unwrap(), "example.service");
    }
}

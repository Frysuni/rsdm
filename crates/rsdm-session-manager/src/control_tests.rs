use std::{os::unix::net::UnixStream, sync::mpsc, thread, time::Duration};

use super::*;

fn connect(uid: u32) -> (Connection, zbus::blocking::Connection, mpsc::Receiver<Request>) {
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let (endpoint, receiver) = Endpoint::channel(uid);
    let server = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server_socket)
            .p2p().server(zbus::Guid::generate()).unwrap()
            .serve_at(OBJECT_PATH, endpoint).unwrap().build().await.unwrap()
    }));
    let client = zbus::blocking::connection::Builder::unix_stream(client_socket)
        .p2p().build().unwrap();
    (server.join().unwrap(), client, receiver)
}

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

#[test]
fn a_pending_launch_does_not_block_a_cancel_request() {
    // SAFETY: geteuid has no preconditions.
    let (_server, connection, requests) = connect(unsafe { libc::geteuid() });
    let launching = connection.clone();
    let worker = thread::spawn(move || {
        proxy(&launching).unwrap().call::<_, _, String>("Launch", &(launch(),)).unwrap()
    });
    let Request::Launch(_, pending) = requests.recv_timeout(Duration::from_secs(2)).unwrap() else { panic!("expected launch"); };
    let cancelling = connection.clone();
    let cancel = thread::spawn(move || {
        proxy(&cancelling).unwrap().call::<_, _, ()>("Cancel", &(launch().generation,)).unwrap()
    });
    let Request::Cancel { reply, .. } = requests.recv_timeout(Duration::from_secs(2)).unwrap() else { panic!("expected cancel"); };
    reply.send_blocking(Ok(())).unwrap();
    cancel.join().unwrap();
    pending.send_blocking(Ok("example.service".into())).unwrap();
    assert_eq!(worker.join().unwrap(), "example.service");
}

#[test]
fn another_uid_cannot_submit_session_requests() {
    // SAFETY: geteuid has no preconditions.
    let other_uid = unsafe { libc::geteuid() }.wrapping_add(1);
    let (_server, connection, requests) = connect(other_uid);
    let result = proxy(&connection).unwrap().call::<_, _, SessionStatus>("Status", &());
    assert!(result.unwrap_err().to_string().contains("another user"));
    assert!(requests.try_recv().is_err());
}

#[test]
fn stop_completion_is_dispatched_before_the_coordinator_can_exit() {
    // SAFETY: geteuid has no preconditions.
    let (_server, connection, requests) = connect(unsafe { libc::geteuid() });
    let worker = thread::spawn(move || {
        proxy(&connection).unwrap().call::<_, _, StopOutcome>("Stop", &(launch().generation, "logout")).unwrap()
    });
    let Request::Stop { reply, .. } = requests.recv_timeout(Duration::from_secs(2)).unwrap() else { panic!("expected stop"); };
    reply.send_blocking(Ok(StopOutcome { result: "completed".into(), forced_units: Vec::new(), message: String::new() })).unwrap();
    assert!(matches!(requests.recv_timeout(Duration::from_secs(2)).unwrap(), Request::StopReplySent(_)));
    assert_eq!(worker.join().unwrap().result, "completed");
}

#[test]
fn oversized_payloads_do_not_reach_the_coordinator() {
    // SAFETY: geteuid has no preconditions.
    let (_server, connection, requests) = connect(unsafe { libc::geteuid() });
    let mut request = launch();
    request.argv.push("x".repeat(64 * 1024 + 1));
    let error = proxy(&connection).unwrap().call::<_, _, String>("Launch", &(request,)).unwrap_err();
    assert!(error.to_string().contains("InvalidArgs"));
    assert!(requests.try_recv().is_err());

    let environment = vec![("NAME".to_string(), "x".repeat(64 * 1024 + 1))];
    let error = proxy(&connection).unwrap().call::<_, _, ()>("Finalize", &(launch().generation, environment)).unwrap_err();
    assert!(error.to_string().contains("InvalidArgs"));
    assert!(requests.try_recv().is_err());
}

#[test]
fn launch_saturation_returns_busy_but_still_admits_cancellation() {
    // SAFETY: geteuid has no preconditions.
    let (_server, connection, requests) = connect(unsafe { libc::geteuid() });
    let mut pending = Vec::new();
    for _ in 0..admission::MAX_REGULAR_REQUESTS {
        let launching = connection.clone();
        let worker = thread::spawn(move || {
            proxy(&launching).unwrap().call::<_, _, String>("Launch", &(launch(),)).unwrap()
        });
        let Request::Launch(_, reply) = requests.recv_timeout(Duration::from_secs(2)).unwrap() else {
            panic!("expected launch");
        };
        pending.push((reply, worker));
    }
    let error = proxy(&connection).unwrap().call::<_, _, String>("Launch", &(launch(),)).unwrap_err();
    assert!(error.to_string().contains("LimitsExceeded"));
    assert!(requests.try_recv().is_err());

    let cancelling = connection.clone();
    let cancel = thread::spawn(move || {
        proxy(&cancelling).unwrap().call::<_, _, ()>("Cancel", &(launch().generation,)).unwrap()
    });
    let Request::Cancel { reply, .. } = requests.recv_timeout(Duration::from_secs(2)).unwrap() else {
        panic!("expected reserved cancellation");
    };
    reply.send_blocking(Ok(())).unwrap();
    cancel.join().unwrap();
    for (reply, worker) in pending {
        reply.send_blocking(Ok("example.service".into())).unwrap();
        assert_eq!(worker.join().unwrap(), "example.service");
    }
}

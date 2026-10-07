use super::*;
use crate::session_manager::control::{Endpoint, Reply, Request};

#[test]
fn expired_shutdown_does_not_wait_for_a_reply_acknowledgement() {
    let mut fixture = Fixture::new();
    fixture.coordinator.replies_pending = 1;
    fixture.coordinator.manager.deadline.set(1);
    let before = Instant::now();
    fixture.coordinator.finish_reply_delivery().unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(fixture.coordinator.replies_pending, 1);
}

#[test]
fn reply_delivery_uses_the_remaining_shutdown_budget() {
    let mut fixture = Fixture::new();
    fixture.coordinator.replies_pending = 1;
    fixture.coordinator.manager.deadline.set(monotonic_usec().unwrap() + 50_000);
    let before = Instant::now();
    fixture.coordinator.finish_reply_delivery().unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(fixture.coordinator.replies_pending, 1);
}

#[test]
fn a_delivered_acknowledgement_finishes_without_waiting_out_the_budget() {
    let mut fixture = Fixture::new();
    // Let the real endpoint send its delivery acknowledgement over the private
    // channel; its permit must stay alive until the coordinator consumes it.
    let (endpoint, requests) = Endpoint::channel(fixture.coordinator.record.identity.uid);
    fixture.coordinator.requests = requests;
    let server = fixture._server.clone();
    async_io::block_on(server.object_server().at(crate::session_manager::control::OBJECT_PATH, endpoint)).unwrap();
    let connection = fixture.coordinator.manager.connection();
    let generation = fixture.coordinator.runtime.generation.clone();
    let stopping = thread::spawn(move || async_io::block_on(async {
        let proxy = zbus::Proxy::new(&connection, crate::session_manager::control::BUS_NAME,
            crate::session_manager::control::OBJECT_PATH, crate::session_manager::control::BUS_NAME).await.unwrap();
        proxy.call::<_, _, StopOutcome>("Stop", &(generation, "logout")).await.unwrap()
    }));
    let Request::Stop { reply, .. } = fixture.coordinator.requests.recv_timeout(Duration::from_secs(1)).unwrap() else {
        panic!("expected stop request");
    };
    reply.try_send(Ok(StopOutcome { result: "completed".into(), forced_units: Vec::new(), message: String::new() })).unwrap();
    fixture.coordinator.replies_pending = 1;
    fixture.coordinator.manager.deadline.set(monotonic_usec().unwrap() + 1_000_000);
    fixture.coordinator.finish_reply_delivery().unwrap();
    assert_eq!(fixture.coordinator.replies_pending, 0);
    assert_eq!(stopping.join().unwrap().result, "completed");
}

#[test]
fn queued_status_requests_still_receive_a_reply_before_the_ack_budget_expires() {
    let mut fixture = Fixture::new();
    let (sender, requests) = std::sync::mpsc::sync_channel(1);
    fixture.coordinator.requests = requests;
    let (reply, response) = async_channel::bounded(1);
    sender.send(Request::Status(Reply::for_test(reply))).unwrap();
    fixture.coordinator.replies_pending = 1;
    fixture.coordinator.manager.deadline.set(monotonic_usec().unwrap() + 50_000);
    fixture.coordinator.finish_reply_delivery().unwrap();
    assert_eq!(response.try_recv().unwrap().unwrap().generation, GENERATION);
}

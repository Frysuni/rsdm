use super::*;
use crate::control::{Endpoint, Reply, Request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_shutdown_does_not_wait_for_a_reply_acknowledgement() {
    let mut fixture = Fixture::new().await;
    fixture.coordinator.replies_pending = 1;
    fixture.coordinator.manager.deadline.set(1);
    let before = Instant::now();
    fixture.coordinator.finish_reply_delivery().await.unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(fixture.coordinator.replies_pending, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reply_delivery_uses_the_remaining_shutdown_budget() {
    let mut fixture = Fixture::new().await;
    fixture.coordinator.replies_pending = 1;
    fixture.coordinator.manager.deadline.set(monotonic_usec().unwrap() + 50_000);
    let before = Instant::now();
    fixture.coordinator.finish_reply_delivery().await.unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(fixture.coordinator.replies_pending, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delivered_acknowledgement_finishes_without_waiting_out_the_budget() {
    let mut fixture = Fixture::new().await;
    // Let the real endpoint send its delivery acknowledgement over the private
    // channel; its permit must stay alive until the coordinator consumes it.
    let (endpoint, requests) = Endpoint::for_test(fixture.coordinator.record.identity.clone()).await;
    fixture.coordinator.requests = requests;
    let server = fixture._server.clone();
    server.object_server().at(crate::control::OBJECT_PATH, endpoint).await.unwrap();
    let connection = fixture.coordinator.manager.connection();
    let generation = fixture.coordinator.runtime.generation.clone();
    let stopping = tokio::spawn(async move {
        let proxy = zbus::Proxy::new(&connection, crate::control::BUS_NAME,
            crate::control::OBJECT_PATH, crate::control::BUS_NAME).await.unwrap();
        proxy.call::<_, _, StopOutcome>("Stop", &(generation, "logout")).await.unwrap()
    });
    let Request::Stop { reply, .. } = tokio::time::timeout(Duration::from_secs(1), fixture.coordinator.requests.recv()).await.unwrap().unwrap() else {
        panic!("expected stop request");
    };
    reply.try_send(Ok(StopOutcome { result: "completed".into(), forced_units: Vec::new(), message: String::new() })).unwrap();
    fixture.coordinator.replies_pending = 1;
    fixture.coordinator.manager.deadline.set(monotonic_usec().unwrap() + 1_000_000);
    fixture.coordinator.finish_reply_delivery().await.unwrap();
    assert_eq!(fixture.coordinator.replies_pending, 0);
    assert_eq!(stopping.await.unwrap().result, "completed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_status_requests_still_receive_a_reply_before_the_ack_budget_expires() {
    let mut fixture = Fixture::new().await;
    let (sender, requests) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.requests = requests;
    let (reply, mut response) = tokio::sync::mpsc::channel(1);
    sender.try_send(Request::Status(Reply::for_test(reply))).unwrap();
    fixture.coordinator.replies_pending = 1;
    fixture.coordinator.manager.deadline.set(monotonic_usec().unwrap() + 50_000);
    fixture.coordinator.finish_reply_delivery().await.unwrap();
    assert_eq!(response.try_recv().unwrap().unwrap().generation, GENERATION);
}

use std::{future::{Future, poll_fn}, task::Poll};

use super::*;
use crate::control::{Endpoint, Request};

#[test]
fn overload_preserves_independent_lifecycle_slots() {
    let admission = Admission::default();
    let regular: Vec<_> = (0..MAX_REGULAR_REQUESTS)
        .map(|_| admission.acquire(Class::Regular).unwrap()).collect();
    assert!(matches!(admission.acquire(Class::Regular), Err(zbus::fdo::Error::LimitsExceeded(_))));
    let stops: Vec<_> = (0..MAX_STOP_REQUESTS)
        .map(|_| admission.acquire(Class::Stop).unwrap()).collect();
    assert!(admission.acquire(Class::Stop).is_err());
    let cancellations: Vec<_> = (0..MAX_CANCEL_REQUESTS)
        .map(|_| admission.acquire(Class::Cancel).unwrap()).collect();
    assert!(admission.acquire(Class::Cancel).is_err());
    let xsmp: Vec<_> = (0..MAX_XSMP_REQUESTS)
        .map(|_| admission.acquire(Class::Xsmp).unwrap()).collect();
    assert!(admission.acquire(Class::Xsmp).is_err());
    drop((regular, stops, cancellations, xsmp));
    assert!(admission.acquire(Class::Regular).is_ok());
    assert!(admission.acquire(Class::Stop).is_ok());
    assert!(admission.acquire(Class::Cancel).is_ok());
    assert!(admission.acquire(Class::Xsmp).is_ok());
}

#[tokio::test]
async fn cancelled_call_futures_cannot_release_queued_work_slots() {
    let (endpoint, mut received) = Endpoint::channel(0);
    let mut held = Vec::new();
    for _ in 0..MAX_REGULAR_REQUESTS {
        let permit = endpoint.admission.acquire(Class::Regular).unwrap();
        let mut call = Box::pin(endpoint.request(permit, Request::Status));
        assert!(poll_fn(|cx| Poll::Ready(call.as_mut().poll(cx))).await.is_pending());
        drop(call);
        let Request::Status(reply) = received.try_recv().unwrap() else {
            panic!("expected queued status request");
        };
        held.push(reply);
    }
    assert!(endpoint.admission.acquire(Class::Regular).is_err());
    let last = held.pop().unwrap();
    let clone = last.clone();
    drop(last);
    assert!(endpoint.admission.acquire(Class::Regular).is_err());
    drop(clone);
    assert!(endpoint.admission.acquire(Class::Regular).is_ok());
}

#[test]
fn concurrent_admission_never_exceeds_the_class_limit() {
    let admission = Admission::default();
    let barrier = std::sync::Barrier::new(9);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8).map(|_| scope.spawn(|| {
            let permits: Vec<_> = (0..16).filter_map(|_| admission.acquire(Class::Regular).ok()).collect();
            barrier.wait();
            barrier.wait();
            permits.len()
        })).collect();
        barrier.wait();
        let pending = admission.regular.load(Ordering::Relaxed);
        let full = admission.acquire(Class::Regular).is_err();
        barrier.wait();
        let granted: usize = workers.into_iter().map(|worker| worker.join().unwrap()).sum();
        assert!(full);
        assert_eq!(pending, MAX_REGULAR_REQUESTS);
        assert_eq!(granted, MAX_REGULAR_REQUESTS);
    });
    assert_eq!(admission.regular.load(Ordering::Relaxed), 0);
}

#[test]
fn stop_acknowledgements_hold_admission_until_received() {
    let (endpoint, mut received) = Endpoint::channel(0);
    let mut held: Vec<_> = (0..MAX_STOP_REQUESTS)
        .map(|_| endpoint.admission.acquire(Class::Stop).unwrap()).collect();
    assert!(endpoint.requests.try_send(Request::StopReplySent(held.pop().unwrap())).is_ok());
    assert!(endpoint.admission.acquire(Class::Stop).is_err());
    drop(received.try_recv().unwrap());
    assert!(endpoint.admission.acquire(Class::Stop).is_ok());
}

#[test]
fn closing_admission_rejects_every_class_and_releases_existing_permits_normally() {
    let admission = Admission::default();
    let clone = admission.clone();
    let held = admission.acquire(Class::Regular).unwrap();
    clone.close();
    for class in [Class::Regular, Class::Stop, Class::Cancel, Class::Xsmp] {
        assert!(matches!(admission.acquire(class), Err(zbus::fdo::Error::Failed(message)) if message.contains("disabled")));
    }
    assert_eq!(admission.regular.load(Ordering::Relaxed), 1);
    drop(held);
    assert_eq!(admission.regular.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn full_queue_returns_backpressure_without_waiting() {
    let (endpoint, _received) = Endpoint::channel(0);
    for _ in 0..QUEUE_CAPACITY {
        let (sender, _result) = tokio::sync::mpsc::channel(1);
        assert!(endpoint.requests.try_send(Request::Status(Reply::for_test(sender))).is_ok());
    }
    let permit = endpoint.admission.acquire(Class::Regular).unwrap();
    let mut call = Box::pin(endpoint.request(permit, Request::Status));
    let result = poll_fn(|cx| Poll::Ready(call.as_mut().poll(cx))).await;
    assert!(matches!(result, Poll::Ready(Err(zbus::fdo::Error::LimitsExceeded(_)))));
    drop(call);
    let _available: Vec<_> = (0..MAX_REGULAR_REQUESTS)
        .map(|_| endpoint.admission.acquire(Class::Regular).unwrap()).collect();
}

#[tokio::test]
async fn disconnected_queue_returns_all_admission_capacity() {
    let (endpoint, received) = Endpoint::channel(0);
    drop(received);
    for _ in 0..MAX_REGULAR_REQUESTS + 1 {
        let permit = endpoint.admission.acquire(Class::Regular).unwrap();
        let result = endpoint.request(permit, Request::Status).await;
        assert!(matches!(result, Err(zbus::fdo::Error::Failed(_))));
    }
    let _available: Vec<_> = (0..MAX_REGULAR_REQUESTS)
        .map(|_| endpoint.admission.acquire(Class::Regular).unwrap()).collect();
}

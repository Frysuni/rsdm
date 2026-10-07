use futures_lite::future;

use super::*;
use crate::session_manager::control::{Endpoint, Request};

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

#[test]
fn cancelled_call_futures_cannot_release_queued_work_slots() {
    async_io::block_on(async {
        let (endpoint, received) = Endpoint::channel(0);
        let mut held = Vec::new();
        for _ in 0..MAX_REGULAR_REQUESTS {
            let permit = endpoint.admission.acquire(Class::Regular).unwrap();
            let mut call = Box::pin(endpoint.request(permit, Request::Status));
            assert!(future::poll_once(call.as_mut()).await.is_none());
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
    });
}

#[test]
fn stop_acknowledgements_hold_admission_until_received() {
    let (endpoint, received) = Endpoint::channel(0);
    let mut held: Vec<_> = (0..MAX_STOP_REQUESTS)
        .map(|_| endpoint.admission.acquire(Class::Stop).unwrap()).collect();
    assert!(endpoint.requests.try_send(Request::StopReplySent(held.pop().unwrap())).is_ok());
    assert!(endpoint.admission.acquire(Class::Stop).is_err());
    drop(received.try_recv().unwrap());
    assert!(endpoint.admission.acquire(Class::Stop).is_ok());
}

#[test]
fn full_queue_returns_backpressure_without_waiting() {
    async_io::block_on(async {
        let (endpoint, _received) = Endpoint::channel(0);
        for _ in 0..QUEUE_CAPACITY {
            let (sender, _result) = async_channel::bounded(1);
            assert!(endpoint.requests.try_send(Request::Status(Reply::for_test(sender))).is_ok());
        }
        let permit = endpoint.admission.acquire(Class::Regular).unwrap();
        let mut call = Box::pin(endpoint.request(permit, Request::Status));
        let result = future::poll_once(call.as_mut()).await;
        assert!(matches!(result, Some(Err(zbus::fdo::Error::LimitsExceeded(_)))));
        drop(call);
        let _available: Vec<_> = (0..MAX_REGULAR_REQUESTS)
            .map(|_| endpoint.admission.acquire(Class::Regular).unwrap()).collect();
    });
}

#[test]
fn disconnected_queue_returns_all_admission_capacity() {
    let (endpoint, received) = Endpoint::channel(0);
    drop(received);
    for _ in 0..MAX_REGULAR_REQUESTS + 1 {
        let permit = endpoint.admission.acquire(Class::Regular).unwrap();
        let result = async_io::block_on(endpoint.request(permit, Request::Status));
        assert!(matches!(result, Err(zbus::fdo::Error::Failed(_))));
    }
    let _available: Vec<_> = (0..MAX_REGULAR_REQUESTS)
        .map(|_| endpoint.admission.acquire(Class::Regular).unwrap()).collect();
}

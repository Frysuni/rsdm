use std::{
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Wake, Waker},
};

use super::*;
use crate::session_manager::processes::monotonic_usec;

#[derive(Default)]
struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) { self.0.fetch_add(1, Ordering::SeqCst); }
}

#[test]
fn an_expired_deadline_never_polls_a_new_operation() {
    let deadline = Deadline::default();
    deadline.set(1);
    let calls = AtomicUsize::new(0);
    let result: zbus::Result<()> = async_io::block_on(deadline.bound(async {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    assert!(matches!(result, Err(error) if super::super::bus::retryable_error(&error)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn installing_a_deadline_wakes_every_pending_operation() {
    let deadline = Deadline::default();
    let mut pending = Vec::new();
    for _ in 0..4 {
        let wakes = Arc::new(Wakes::default());
        let waker = Waker::from(wakes.clone());
        let mut operation = Box::pin(deadline.bound(std::future::pending::<zbus::Result<()>>()));
        assert!(matches!(operation.as_mut().poll(&mut Context::from_waker(&waker)), Poll::Pending));
        pending.push((operation, wakes, waker));
    }
    deadline.set(1);
    for (mut operation, wakes, waker) in pending {
        assert!(wakes.0.load(Ordering::SeqCst) > 0, "deadline did not wake this operation");
        assert!(matches!(operation.as_mut().poll(&mut Context::from_waker(&waker)), Poll::Ready(Err(_))));
    }
}

#[test]
fn shortening_a_deadline_interrupts_an_existing_wait() {
    let deadline = Deadline::default();
    deadline.set(monotonic_usec().unwrap() + 10_000_000);
    let mut operation = Box::pin(deadline.bound(std::future::pending::<zbus::Result<()>>()));
    assert!(async_io::block_on(future::poll_once(operation.as_mut())).is_none());
    deadline.set(1);
    assert!(async_io::block_on(operation).is_err());
}

#[test]
fn revocation_removes_the_old_timer_without_cancelling_the_operation() {
    let deadline = Deadline::default();
    deadline.set(monotonic_usec().unwrap() + 50_000);
    let mut operation = Box::pin(deadline.bound(async {
        async_io::Timer::after(Duration::from_millis(100)).await;
        Ok::<_, zbus::Error>(7)
    }));
    assert!(async_io::block_on(future::poll_once(operation.as_mut())).is_none());
    deadline.set(0);
    assert_eq!(async_io::block_on(operation).unwrap(), 7);
}

#[test]
fn completed_and_dropped_operations_release_their_deadline_watchers() {
    let deadline = Deadline::default();
    for _ in 0..64 {
        let mut operation = Box::pin(deadline.bound(std::future::pending::<zbus::Result<()>>()));
        assert!(async_io::block_on(future::poll_once(Pin::as_mut(&mut operation))).is_none());
        drop(operation);
        assert_eq!(deadline.state.lock().unwrap().changed.1.receiver_count(), 1);
        assert!(async_io::block_on(deadline.bound(async { Ok::<_, zbus::Error>(()) })).is_ok());
        assert_eq!(deadline.state.lock().unwrap().changed.1.receiver_count(), 1);
    }
}

#[test]
fn remaining_time_is_shared_and_cannot_exceed_the_local_limit() {
    let deadline = Deadline::default();
    let clone = deadline.clone();
    assert_eq!(clone.remaining(Duration::from_secs(2)).unwrap(), Duration::from_secs(2));
    deadline.set(monotonic_usec().unwrap() + 1_000_000);
    assert_eq!(clone.remaining(Duration::from_millis(10)).unwrap(), Duration::from_millis(10));
    assert!(clone.remaining(Duration::from_secs(5)).unwrap() <= Duration::from_secs(1));
    deadline.set(1);
    assert!(clone.remaining(Duration::from_secs(5)).is_err());
}

#[test]
fn a_phase_reserves_time_without_changing_the_absolute_deadline() {
    let deadline = Deadline::default();
    let hard = monotonic_usec().unwrap() + 10_000_000;
    deadline.set(hard);
    let phase = deadline.reserving(Duration::from_secs(2)).unwrap();
    assert_eq!(phase.get(), hard - 2_000_000);
    assert_eq!(deadline.get(), hard);
    deadline.set(monotonic_usec().unwrap() + 1_000_000);
    assert!(async_io::block_on(phase.bound(async { Ok::<_, zbus::Error>(()) })).is_err());
    assert!(deadline.remaining(Duration::MAX).is_ok());
}

#[test]
fn a_short_budget_keeps_time_for_both_phases() {
    let deadline = Deadline::default();
    let now = monotonic_usec().unwrap();
    deadline.set(now + 1_000_000);
    let phase = deadline.reserving(Duration::from_secs(2)).unwrap();
    assert!(phase.get() >= now + 500_000);
    assert!(phase.get() < deadline.get());
    assert!(phase.remaining(Duration::MAX).is_ok());
}

#[test]
fn a_phase_created_before_shutdown_tracks_installation_and_revocation() {
    let deadline = Deadline::default();
    let phase = deadline.reserving(Duration::from_secs(2)).unwrap();
    assert_eq!(phase.get(), 0);
    let mut operation = Box::pin(phase.bound(std::future::pending::<zbus::Result<()>>()));
    assert!(async_io::block_on(future::poll_once(operation.as_mut())).is_none());
    deadline.set(monotonic_usec().unwrap() + 1_000_000);
    assert!(async_io::block_on(operation).is_err());
    deadline.set(0);
    assert_eq!(phase.get(), 0);
    assert!(async_io::block_on(phase.bound(async { Ok::<_, zbus::Error>(()) })).is_ok());
}

#[test]
fn revising_a_phase_interrupts_its_wait_and_revocation_removes_its_timer() {
    let deadline = Deadline::default();
    deadline.set(monotonic_usec().unwrap() + 10_000_000);
    let phase = deadline.reserving(Duration::from_secs(2)).unwrap();
    let mut operation = Box::pin(phase.bound(std::future::pending::<zbus::Result<()>>()));
    assert!(async_io::block_on(future::poll_once(operation.as_mut())).is_none());
    deadline.set(monotonic_usec().unwrap() + 1_000_000);
    assert!(async_io::block_on(operation).is_err());

    deadline.set(monotonic_usec().unwrap() + 2_050_000);
    let mut operation = Box::pin(phase.bound(async {
        async_io::Timer::after(Duration::from_millis(100)).await;
        Ok::<_, zbus::Error>(())
    }));
    assert!(async_io::block_on(future::poll_once(operation.as_mut())).is_none());
    deadline.set(0);
    assert!(async_io::block_on(operation).is_ok());
}

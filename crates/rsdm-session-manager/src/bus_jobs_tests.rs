//! Lost replies and signals must not cause a second systemd mutation.

use std::{sync::atomic::Ordering, thread};

use super::*;

use super::fixture::{fixture, state};

#[test]
fn accepted_start_survives_lost_reply_or_job_signal_without_replay() {
    for lost_reply in [true, false] {
        let (manager, _server, state) = fixture(state(lost_reply, true, 0));
        let properties = vec![("Environment", Value::new(vec!["RSDM_SESSION_GENERATION=ours".to_string()]))];
        async_io::block_on(start(&manager, "example.service", &properties, Duration::from_millis(20), &Cell::new(false))).unwrap();
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn lost_reply_accepts_only_a_successfully_exited_service() {
    for status in [0, 1] {
        let (manager, _server, state) = fixture(state(true, false, status));
        let result = async_io::block_on(start(&manager, "example.service", &Vec::new(), Duration::from_millis(50), &Cell::new(false)));
        assert_eq!(result.is_ok(), status == 0);
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn an_existing_or_foreign_unit_cannot_confirm_a_lost_start_reply() {
    for existing in [true, false] {
        let mut initial = state(true, true, 0);
        initial.exists.store(existing, Ordering::SeqCst);
        if !existing { initial.generation = "foreign"; }
        let (manager, _server, state) = fixture(initial);
        let properties = vec![("Environment", Value::new(vec!["RSDM_SESSION_GENERATION=ours".to_string()]))];
        assert!(async_io::block_on(start(&manager, "example.service", &properties, Duration::from_millis(50), &Cell::new(false))).is_err());
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn a_lost_stop_reply_requires_an_empty_stopped_unit() {
    for active in [false, true] {
        let initial = state(true, active, 0);
        initial.exists.store(true, Ordering::SeqCst);
        let (manager, _server, state) = fixture(initial);
        let result = async_io::block_on(stop(&manager, "example.service", Duration::from_millis(50)));
        assert_eq!(result.is_ok(), !active);
        if active {
            assert!(matches!(&result, Err(SessionError::Bus(error)) if retryable_error(error)));
        }
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn the_operation_budget_includes_preflight_reads() {
    let mut initial = state(false, false, 0);
    initial.exists.store(true, Ordering::SeqCst);
    initial.preflight_delay = Duration::from_secs(2);
    let (manager, _server, state) = fixture(initial);
    let before = Instant::now();
    let result = async_io::block_on(stop(&manager, "example.service", Duration::from_millis(50)));
    assert!(matches!(&result, Err(SessionError::Bus(error)) if retryable_error(error)));
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_accepted_mutation_cannot_wait_past_its_budget_for_the_reply() {
    let mut initial = state(false, true, 0);
    initial.reply_delay = Duration::from_secs(2);
    let (manager, _server, state) = fixture(initial);
    let before = Instant::now();
    let result = async_io::block_on(start(&manager, "example.service", &Vec::new(), Duration::from_millis(50), &Cell::new(false)));
    assert!(matches!(&result, Err(SessionError::Bus(error)) if retryable_error(error)));
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    assert!(state.exists.load(Ordering::SeqCst));
}

#[test]
fn a_missing_job_signal_does_not_get_an_additional_inspection_budget() {
    let mut initial = state(false, false, 0);
    initial.exists.store(true, Ordering::SeqCst);
    initial.inspection_delay = Duration::from_secs(2);
    let (manager, _server, state) = fixture(initial);
    let before = Instant::now();
    let result = async_io::block_on(stop(&manager, "example.service", Duration::from_millis(50)));
    assert!(matches!(&result, Err(SessionError::Bus(error)) if retryable_error(error)));
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn an_expired_budget_does_not_send_a_mutation() {
    let initial = state(false, false, 0);
    let (manager, _server, state) = fixture(initial);
    assert!(async_io::block_on(start(&manager, "example.service", &Vec::new(), Duration::ZERO, &Cell::new(false))).is_err());
    assert!(async_io::block_on(stop(&manager, "example.service", Duration::ZERO)).is_err());
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    assert!(!state.exists.load(Ordering::SeqCst));
}

#[test]
fn a_missing_job_signal_and_unfinished_unit_cannot_extend_the_stop_budget() {
    let initial = state(false, true, 0);
    initial.exists.store(true, Ordering::SeqCst);
    let (manager, _server, state) = fixture(initial);
    let before = Instant::now();
    let result = async_io::block_on(stop(&manager, "example.service", Duration::from_millis(50)));
    assert!(matches!(&result, Err(SessionError::Bus(error)) if retryable_error(error)));
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn manager_clones_share_the_shutdown_deadline_and_its_revocation() {
    let initial = state(false, false, 0);
    initial.exists.store(true, Ordering::SeqCst);
    let (manager, _server, state) = fixture(initial);
    let clone = manager.clone();
    clone.deadline.set(1);
    assert!(manager.stop("example.service", Duration::from_secs(15)).is_err());
    assert!(manager.start_service("example.service", &Vec::new()).is_err());
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    clone.deadline.set(0);
    manager.stop("example.service", Duration::from_secs(1)).unwrap();
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_new_shutdown_deadline_interrupts_a_read_already_waiting_for_dbus() {
    let (manager, _server, _state) = fixture(state(false, false, 0));
    let deadline = manager.deadline.clone();
    let (started, received) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || manager.read(|| async {
        started.send(()).unwrap();
        std::future::pending::<zbus::Result<()>>().await
    }));
    received.recv_timeout(Duration::from_secs(1)).unwrap();
    let before = Instant::now();
    deadline.set(1);
    assert!(worker.join().unwrap().is_err());
    assert!(before.elapsed() < Duration::from_secs(1));
}

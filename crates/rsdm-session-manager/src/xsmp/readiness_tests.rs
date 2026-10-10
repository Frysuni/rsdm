use std::{io::Write, os::{fd::AsRawFd, unix::net::UnixStream}, time::Duration};

use super::*;

fn flags(socket: &UnixStream) -> libc::c_int {
    unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_GETFL) }
}

#[tokio::test]
async fn waiting_does_not_block_timers_and_cancellation_restores_socket_flags() {
    let (reader, _writer) = UnixStream::pair().unwrap();
    let original = flags(&reader);
    let descriptors = [reader.as_raw_fd()];
    let mut waiting = Box::pin(wait(&descriptors));
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(10)) => {},
        result = &mut waiting => panic!("idle socket became ready: {result:?}"),
    }
    assert_ne!(flags(&reader) & libc::O_NONBLOCK, 0);
    drop(waiting);
    assert_eq!(flags(&reader), original);
}

#[tokio::test]
async fn readiness_selects_the_live_socket_and_restores_every_descriptors_flags() {
    let (idle, _idle_writer) = UnixStream::pair().unwrap();
    let (reader, mut writer) = UnixStream::pair().unwrap();
    let original = flags(&reader);
    writer.write_all(b"message").unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(1), wait(&[idle.as_raw_fd(), reader.as_raw_fd()]))
        .await.unwrap().unwrap();
    assert_eq!(ready, 1);
    assert_eq!(flags(&reader), original);
    assert_eq!(flags(&idle) & libc::O_NONBLOCK, 0);
}

#[tokio::test]
async fn closed_connections_are_reported_without_waiting_for_a_periodic_tick() {
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(writer);
    assert_eq!(tokio::time::timeout(Duration::from_secs(1), wait(&[reader.as_raw_fd()]))
        .await.unwrap().unwrap(), 0);
    assert_eq!(flags(&reader) & libc::O_NONBLOCK, 0);
}

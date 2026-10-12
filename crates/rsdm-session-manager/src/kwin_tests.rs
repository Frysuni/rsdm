//! Live resources, framing and PID changes constrain the concrete close request.

use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::UnixStream};

use super::*;
use crate::{wayfire::tests::Fixture, wayland_registry::tests::{advertisement, event, read_requests}};

async fn request(peer: &mut UnixStream) -> (u32, u16, Vec<u8>) {
    let mut header = [0_u8; 8];
    peer.read_exact(&mut header).await.unwrap();
    let object = u32::from_ne_bytes(header[..4].try_into().unwrap());
    let word = u32::from_ne_bytes(header[4..].try_into().unwrap());
    let mut body = vec![0; (word >> 16) as usize - 8];
    peer.read_exact(&mut body).await.unwrap();
    (object, word as u16, body)
}

async fn sync(peer: &mut UnixStream) {
    let (object, opcode, body) = request(peer).await;
    assert_eq!((object, opcode), (1, 0));
    let callback = u32::from_ne_bytes(body.try_into().unwrap());
    peer.write_all(&event(callback, 0, &0_u32.to_ne_bytes())).await.unwrap();
    peer.write_all(&event(1, 1, &callback.to_ne_bytes())).await.unwrap();
}

async fn snapshot(peer: &mut UnixStream) -> Vec<u32> {
    read_requests(peer).await;
    peer.write_all(&advertisement(9, INTERFACE, 21)).await.unwrap();
    peer.write_all(&event(3, 0, &0_u32.to_ne_bytes())).await.unwrap();
    let (object, opcode, body) = request(peer).await;
    assert_eq!((object, opcode), (2, 0));
    assert_eq!(&body[..4], 9_u32.to_ne_bytes());
    assert_eq!(&body[body.len() - 8..], [17_u32.to_ne_bytes(), MANAGEMENT.to_ne_bytes()].concat());
    for uuid in ["parent", "child"] {
        let mut body = 1_u32.to_ne_bytes().to_vec();
        body.extend(string(uuid));
        peer.write_all(&event(MANAGEMENT, 4, &body)).await.unwrap();
    }
    sync(peer).await;
    let mut handles = Vec::new();
    for _ in 0..2 {
        let (object, opcode, body) = request(peer).await;
        assert_eq!((object, opcode), (MANAGEMENT, 2));
        let id = u32::from_ne_bytes(body[..4].try_into().unwrap());
        handles.push(id);
        for frame in [event(id, 10, &std::process::id().to_ne_bytes()), event(id, 7, &handles.first().filter(|_| handles.len() == 2).copied().unwrap_or(0).to_ne_bytes()),
            event(id, 2, &0x100_u32.to_ne_bytes()), event(id, 6, &[])] {
            for byte in frame { peer.write_all(&[byte]).await.unwrap(); }
        }
    }
    sync(peer).await;
    handles
}

#[tokio::test]
async fn one_live_handle_is_closed_after_pid_and_parent_discovery() {
    let fixture = Fixture::new();
    let mut connection = Connection::connect(&fixture.path, unsafe { libc::geteuid() }).await.unwrap();
    let mut peer = fixture.accept().await;
    let server = tokio::spawn(async move {
        let handles = snapshot(&mut peer).await;
        let close = request(&mut peer).await;
        assert_eq!(close, (handles[0], 4, Vec::new()));
        sync(&mut peer).await;
        peer
    });
    let mut windows = connection.windows().await.unwrap();
    windows.reverse();
    connection.order_windows(&mut windows);
    assert_eq!(windows.len(), 2);
    assert!(!connection.state.has_parent(windows[0].id));
    assert!(connection.state.has_parent(windows[1].id));
    assert!(connection.close(windows[0].id).await.unwrap());
    let _peer = server.await.unwrap();
}

#[tokio::test]
async fn a_lost_acknowledgement_keeps_one_close_unconfirmed() {
    let fixture = Fixture::new();
    let mut connection = Connection::connect(&fixture.path, unsafe { libc::geteuid() }).await.unwrap();
    let mut peer = fixture.accept().await;
    let server = tokio::spawn(async move {
        let handles = snapshot(&mut peer).await;
        assert_eq!(request(&mut peer).await, (handles[0], 4, Vec::new()));
        assert_eq!(request(&mut peer).await.0, 1);
    });
    let windows = connection.windows().await.unwrap();
    assert!(connection.close(windows[0].id).await.is_err());
    server.await.unwrap();
}

#[test]
fn incomplete_unmapped_and_changed_pid_resources_do_not_authorize_the_original_window() {
    let mut state = State::default();
    state.insert(6).unwrap();
    state.event(6, 2, &0x100_u32.to_ne_bytes()).unwrap();
    state.event(6, 10, &123_u32.to_ne_bytes()).unwrap();
    assert!(state.windows().is_empty());
    state.event(6, 6, &[]).unwrap();
    let original = state.windows().pop().unwrap();
    state.event(6, 10, &124_u32.to_ne_bytes()).unwrap();
    assert!(!state.windows().contains(&original));
    state.event(6, 5, &[]).unwrap();
    state.event(6, 10, &123_u32.to_ne_bytes()).unwrap();
    state.event(6, 6, &[]).unwrap();
    assert!(state.windows().is_empty());
}

#[test]
fn queued_incomplete_events_and_uuid_reuse_are_rejected() {
    for frames in [vec![0], event(1, 0, &[]), event(7, 10, &1_u32.to_ne_bytes()), event(4, 4, &[])] {
        assert!(State::default().pending_events(&frames).is_err());
    }
    let mut state = State::default();
    let mut announcement = 1_u32.to_ne_bytes().to_vec();
    announcement.extend(string("uuid"));
    state.event(4, 4, &announcement).unwrap();
    assert!(state.event(4, 4, &announcement).is_err());
}

#[test]
fn old_unknown_and_malformed_versions_do_not_establish_trusted_window_pids() {
    for version in ["", "KWin version: 6.3.6", "KWin version: 5.27.0", "KWin version: 7.0.0", "KWin version: 6.unknown"] {
        assert!(!trusted_version(version), "{version}");
    }
    for version in ["KWin version: 6.4.0", "Version\nKWin version: 6.7.1\n"] { assert!(trusted_version(version)); }
}

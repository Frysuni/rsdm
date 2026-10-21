//! Registry framing and advertisement cannot turn discovery into a close request.

use super::*;
use crate::wayfire::tests::Fixture;

pub(crate) fn event(object: u32, opcode: u16, body: &[u8]) -> Vec<u8> {
    let mut frame = object.to_ne_bytes().to_vec();
    frame.extend_from_slice(&(((body.len() as u32 + 8) << 16) | u32::from(opcode)).to_ne_bytes());
    frame.extend_from_slice(body);
    frame
}

pub(crate) fn advertisement(id: u32, interface: &str, version: u32) -> Vec<u8> {
    let mut body = id.to_ne_bytes().to_vec();
    body.extend_from_slice(&(interface.len() as u32 + 1).to_ne_bytes());
    body.extend_from_slice(interface.as_bytes());
    body.push(0);
    while body.len() % 4 != 0 { body.push(0xff); }
    body.extend_from_slice(&version.to_ne_bytes());
    event(2, 0, &body)
}

pub(crate) async fn read_requests(peer: &mut UnixStream) {
    let mut requests = [0_u8; 24];
    peer.read_exact(&mut requests).await.unwrap();
    assert_eq!(&requests[..12], event(1, 1, &2_u32.to_ne_bytes()));
    assert_eq!(&requests[12..], event(1, 0, &3_u32.to_ne_bytes()));
}

async fn connect(fixture: &Fixture) -> (Connection, UnixStream) {
    // SAFETY: geteuid has no preconditions.
    let connection = Connection::connect(&fixture.path, unsafe { libc::geteuid() }).await.unwrap();
    (connection, fixture.accept().await)
}

#[tokio::test]
async fn fragmented_globals_nonzero_padding_and_removals_are_observed_without_binding() {
    let fixture = Fixture::new();
    let (mut connection, mut peer) = connect(&fixture).await;
    let server = tokio::spawn(async move {
        read_requests(&mut peer).await;
        for frame in [advertisement(4, INTERFACES[0], 1), advertisement(5, INTERFACES[1], 3),
            advertisement(6, INTERFACES[2], 19), advertisement(7, "river_window_manager_v1", 1),
            event(2, 1, &5_u32.to_ne_bytes()), event(3, 0, &0_u32.to_ne_bytes())]
        { for byte in frame { peer.write_all(&[byte]).await.unwrap(); } }
        let mut byte = [0_u8; 1];
        assert_eq!(peer.read(&mut byte).await.unwrap(), 0);
    });
    assert_eq!(connection.discover().await.unwrap(), [format!("{} v1", INTERFACES[0]), format!("{} v19", INTERFACES[2])]);
    drop(connection);
    server.await.unwrap();
}

#[tokio::test]
async fn malformed_and_incomplete_events_fail_without_authorizing_a_backend() {
    let bad_version = advertisement(1, INTERFACES[0], 0);
    let mut bad_string = advertisement(1, INTERFACES[0], 1);
    bad_string[16] = 0;
    let duplicate = [advertisement(1, INTERFACES[0], 1), advertisement(1, INTERFACES[0], 1)].concat();
    for frame in [event(1, 0, &[]), event(9, 0, &[]), event(2, 1, &[]), event(3, 0, &[]),
        vec![2, 0, 0], event(2, 0, &[0; 4]), bad_version, bad_string, duplicate]
    {
        let fixture = Fixture::new();
        let (mut connection, mut peer) = connect(&fixture).await;
        let server = tokio::spawn(async move { read_requests(&mut peer).await; peer.write_all(&frame).await.unwrap(); });
        assert!(connection.discover().await.is_err());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn a_stalled_registry_is_bounded_and_an_event_flood_is_rejected() {
    let fixture = Fixture::new();
    let (mut connection, mut peer) = connect(&fixture).await;
    let server = tokio::spawn(async move {
        read_requests(&mut peer).await;
        let interface = "x".repeat(60_000);
        for id in 1..=20 {
            if peer.write_all(&advertisement(id, &interface, 1)).await.is_err() { break; }
        }
    });
    assert!(connection.discover().await.unwrap_err().to_string().contains("oversized"));
    drop(connection);
    server.await.unwrap();
    let (mut connection, _peer) = connect(&fixture).await;
    assert!(connection.discover().await.unwrap_err().to_string().contains("timed out"));
}

#[test]
fn endpoint_resolution_uses_only_the_recorded_generation_environment() {
    assert_eq!(socket_path(&[("WAYLAND_DISPLAY".into(), "/tmp/display".into())]), Some("/tmp/display".into()));
    assert_eq!(socket_path(&[("WAYLAND_DISPLAY".into(), "wayland-0".into()), ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into())]), Some("/run/user/1000/wayland-0".into()));
    for display in ["", "../wayland-0", "./wayland-0"] {
        assert!(socket_path(&[("WAYLAND_DISPLAY".into(), display.into()), ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into())]).is_none());
    }
    assert!(socket_path(&[("WAYLAND_DISPLAY".into(), "wayland-0".into())]).is_none());
}

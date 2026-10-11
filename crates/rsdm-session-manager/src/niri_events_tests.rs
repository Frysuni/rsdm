//! The wire event stream must veto stale targets without replacing them.

use super::*;
use crate::compositor_socket::MAX_FRAME_BYTES;
use crate::niri::tests::{Fixture, read_request};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

async fn subscribe(fixture: &Fixture) -> (EventStream, BufReader<UnixStream>, Vec<Window>) {
    let (connection, mut peer) = fixture.connect().await;
    let server = tokio::spawn(async move {
        assert_eq!(read_request(&mut peer).await, json!("EventStream"));
        peer.get_mut().write_all(b"{\"Ok\":\"Handled\"}\n{\"WorkspacesChanged\":{\"workspaces\":[]}}\n{\"WindowsChanged\":{\"windows\":[{\"id\":42,\"pid\":123},{\"id\":7,\"pid\":null}]}}\n").await.unwrap();
        peer
    });
    let (events, windows) = EventStream::subscribe(connection).await.unwrap();
    (events, server.await.unwrap(), windows)
}

#[tokio::test]
async fn initial_snapshot_and_unrelated_events_preserve_the_selected_target() {
    let fixture = Fixture::new();
    let (mut events, mut peer, windows) = subscribe(&fixture).await;
    assert_eq!(windows, [Window { id: 42, pid: Some(123) }, Window { id: 7, pid: None }]);
    assert!(events.unchanged(&windows[0]).unwrap());
    peer.get_mut().write_all(b"{\"WindowClosed\":{\"id\":7}}\n{\"WindowFocusChanged\":{\"id\":42}}\n{\"FutureEvent\":{}}\n{\"WindowOpenedOrChanged\":{\"window\":{\"id\":42,\"pid\":123,\"title\":\"changed\"}}}\n").await.unwrap();
    assert!(events.unchanged(&windows[0]).unwrap());
}

#[tokio::test]
async fn closed_replaced_or_missing_targets_are_not_reauthorized_by_later_events() {
    for changed in [
        json!({"WindowClosed":{"id":42}}),
        json!({"WindowOpenedOrChanged":{"window":{"id":42,"pid":456}}}),
        json!({"WindowsChanged":{"windows":[]}}),
    ] {
        let fixture = Fixture::new();
        let (mut events, mut peer, windows) = subscribe(&fixture).await;
        let restored = json!({"WindowOpenedOrChanged":{"window":{"id":42,"pid":123}}});
        peer.get_mut().write_all(format!("{changed}\n{restored}\n").as_bytes()).await.unwrap();
        assert!(!events.unchanged(&windows[0]).unwrap());
    }
}

#[tokio::test]
async fn disconnected_partial_and_invalid_events_cannot_authorize_close() {
    for frame in [b"{\"WindowClosed\":{\"id\":42}}".to_vec(), b"invalid\n".to_vec(), b"null\n".to_vec(), b"{\"WindowClosed\":{}}\n".to_vec()] {
        let fixture = Fixture::new();
        let (mut events, mut peer, windows) = subscribe(&fixture).await;
        peer.get_mut().write_all(&frame).await.unwrap();
        assert!(events.unchanged(&windows[0]).is_err());
    }
    let fixture = Fixture::new();
    let (mut events, peer, windows) = subscribe(&fixture).await;
    drop(peer);
    assert!(events.unchanged(&windows[0]).is_err());
}

#[tokio::test]
async fn rejected_or_silent_subscriptions_are_bounded() {
    let fixture = Fixture::new();
    let (connection, mut peer) = fixture.connect().await;
    let server = tokio::spawn(async move {
        read_request(&mut peer).await;
        peer.get_mut().write_all(b"{\"Err\":\"unsupported\"}\n").await.unwrap();
    });
    assert!(EventStream::subscribe(connection).await.is_err());
    server.await.unwrap();
    let (connection, _peer) = fixture.connect().await;
    let before = std::time::Instant::now();
    assert!(EventStream::subscribe(connection).await.is_err());
    assert!(before.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn an_acknowledged_subscription_must_publish_a_bounded_valid_snapshot() {
    for snapshot in [b"{\"WindowsChanged\":{\"windows\":false}}\n".to_vec(), vec![b'x'; MAX_FRAME_BYTES as usize + 1]] {
        let fixture = Fixture::new();
        let (connection, mut peer) = fixture.connect().await;
        let server = tokio::spawn(async move {
            read_request(&mut peer).await;
            peer.get_mut().write_all(b"{\"Ok\":\"Handled\"}\n").await.unwrap();
            let _ = peer.get_mut().write_all(&snapshot).await;
        });
        assert!(EventStream::subscribe(connection).await.is_err());
        server.await.unwrap();
    }
    let fixture = Fixture::new();
    let (connection, mut peer) = fixture.connect().await;
    let server = tokio::spawn(async move {
        read_request(&mut peer).await;
        peer.get_mut().write_all(b"{\"Ok\":\"Handled\"}\n").await.unwrap();
        peer
    });
    let before = std::time::Instant::now();
    assert!(EventStream::subscribe(connection).await.is_err());
    assert!(before.elapsed() < Duration::from_secs(3));
    server.await.unwrap();
}

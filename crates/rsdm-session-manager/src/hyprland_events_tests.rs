//! Replaced addresses and incomplete event data veto the selected stable ID.

use super::*;
use crate::hyprland::tests::{Fixture, client};
use tokio::io::AsyncWriteExt;

async fn subscribe(fixture: &Fixture) -> (EventStream, UnixStream) {
    let server = fixture.serve(vec!["ok".into(), serde_json::json!([client("1","0x123",123)]).to_string()]);
    let connection = fixture.connect().await;
    let (events, windows) = EventStream::subscribe(connection).await.unwrap();
    assert_eq!(windows, [Window { id:1, pid:Some(123) }]);
    let (peer, _) = fixture.events.accept().await.unwrap();
    assert_eq!(server.await.unwrap().len(), 2);
    (events, peer)
}

#[tokio::test]
async fn unrelated_events_preserve_the_target_and_unknown_ids_cannot_authorize_close() {
    let fixture = Fixture::new();
    let (mut events, mut peer) = subscribe(&fixture).await;
    assert!(events.unchanged(&Window { id:1, pid:Some(123) }).unwrap());
    peer.write_all(b"closewindow>>456\nactivewindowv2>>123\n").await.unwrap();
    assert!(events.unchanged(&Window { id:1, pid:Some(123) }).unwrap());
    assert!(!events.unchanged(&Window { id:2, pid:Some(123) }).unwrap());
}

#[tokio::test]
async fn closed_or_reused_addresses_are_never_restored_by_later_events() {
    for change in [b"closewindow>>123\nactivewindowv2>>123\n".as_slice(), b"openwindow>>123,1,app,title\n".as_slice()] {
        let fixture = Fixture::new();
        let (mut events, mut peer) = subscribe(&fixture).await;
        peer.write_all(change).await.unwrap();
        assert!(!events.unchanged(&Window { id:1, pid:Some(123) }).unwrap());
    }
}

#[tokio::test]
async fn incomplete_malformed_events_and_disconnects_cannot_authorize_close() {
    for response in [b"closewindow>>123".as_slice(), b"broken\n".as_slice(), b"closewindow>>invalid\n".as_slice(), &[0xff,b'\n']] {
        let fixture = Fixture::new();
        let (mut events, mut peer) = subscribe(&fixture).await;
        peer.write_all(response).await.unwrap();
        assert!(events.unchanged(&Window { id:1, pid:Some(123) }).is_err());
    }
    let fixture = Fixture::new();
    let (mut events, peer) = subscribe(&fixture).await;
    drop(peer);
    assert!(events.unchanged(&Window { id:1, pid:Some(123) }).is_err());
}

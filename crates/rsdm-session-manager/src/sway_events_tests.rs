//! Stale containers and interrupted event frames cannot authorize a close.

use serde_json::json;
use tokio::{io::AsyncWriteExt, net::UnixStream};

use super::*;
use crate::sway::tests::{Fixture, frame, request};

async fn subscribe(fixture: &Fixture) -> (EventStream, UnixStream) {
    let (connection, mut peer) = fixture.connect().await;
    let server = tokio::spawn(async move {
        assert_eq!(request(&mut peer).await, (2, br#"["window"]"#.to_vec()));
        peer.write_all(&frame(2, &json!({"success":true}))).await.unwrap();
        peer
    });
    let events = EventStream::subscribe(connection).await.unwrap();
    (events, server.await.unwrap())
}

#[tokio::test]
async fn unrelated_events_and_same_owner_changes_preserve_the_target() {
    let fixture = Fixture::new();
    let (mut events, mut peer) = subscribe(&fixture).await;
    let selected = Window { id:42, pid:Some(123) };
    assert!(events.unchanged(&selected).unwrap());
    for (change, id) in [("close",7),("focus",42),("title",42)] {
        peer.write_all(&frame(0x80000003, &json!({"change":change,"container":{"id":id,"pid":123,"shell":"xdg_shell"}}))).await.unwrap();
    }
    assert!(events.unchanged(&selected).unwrap());
}

#[tokio::test]
async fn closed_replaced_or_untrusted_targets_cannot_be_restored_in_the_same_burst() {
    for (change, pid, shell) in [("close",123,"xdg_shell"),("new",123,"xdg_shell"),("title",456,"xdg_shell"),("title",123,"xwayland")] {
        let fixture = Fixture::new();
        let (mut events, mut peer) = subscribe(&fixture).await;
        let changed = frame(0x80000003, &json!({"change":change,"container":{"id":42,"pid":pid,"shell":shell}}));
        let restored = frame(0x80000003, &json!({"change":"title","container":{"id":42,"pid":123,"shell":"xdg_shell"}}));
        peer.write_all(&[changed, restored].concat()).await.unwrap();
        assert!(!events.unchanged(&Window { id:42, pid:Some(123) }).unwrap());
    }
}

#[tokio::test]
async fn partial_invalid_foreign_event_types_and_disconnects_are_not_success() {
    let mut partial = frame(0x80000003, &json!({"change":"close","container":{"id":42}}));
    partial.pop();
    for response in [partial, vec![b'x';HEADER_BYTES], frame(0x80000000, &json!({})), frame(0x80000003, &json!({}))] {
        let fixture = Fixture::new();
        let (mut events, mut peer) = subscribe(&fixture).await;
        peer.write_all(&response).await.unwrap();
        assert!(events.unchanged(&Window { id:42, pid:Some(123) }).is_err());
    }
    let fixture = Fixture::new();
    let (mut events, peer) = subscribe(&fixture).await;
    drop(peer);
    assert!(events.unchanged(&Window { id:42, pid:Some(123) }).is_err());
}

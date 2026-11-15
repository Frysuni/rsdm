//! Systemd may collect a unit between any two ownership reads.

use std::{future::poll_fn, pin::Pin};

use zbus::{MessageStream, export::futures_core::Stream};

use super::*;

async fn fixture(missing_at: usize, denied: bool) -> (Connection, tokio::task::JoinHandle<usize>) {
    let (server, client) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server.into_std().unwrap()).p2p()
            .server(zbus::Guid::generate()).build().await.unwrap()
    });
    let client = zbus::connection::Builder::unix_stream(client.into_std().unwrap()).p2p().build().await.unwrap();
    let server = server.await.unwrap();
    let worker = tokio::spawn(async move {
        let mut messages = MessageStream::from(&server);
        for read in 1..=missing_at {
            let call = tokio::time::timeout(Duration::from_secs(2),
                poll_fn(|cx| Pin::new(&mut messages).poll_next(cx))).await.unwrap().unwrap().unwrap();
            let header = call.header();
            if read == missing_at {
                let name = if denied { "org.freedesktop.DBus.Error.AccessDenied" }
                    else { "org.freedesktop.systemd1.NoSuchUnit" };
                server.reply_error(&header, name, &"ownership read failed").await.unwrap();
                return read;
            }
            match header.member().unwrap().as_str() {
                "GetUnit" => server.reply(&header, &OwnedObjectPath::try_from("/unit").unwrap()).await.unwrap(),
                "GetUnitProcesses" => server.reply(&header, &Vec::<(String, u32, String)>::new()).await.unwrap(),
                "Get" => {
                    let body = call.body();
                    let (_, property): (&str, &str) = body.deserialize().unwrap();
                    let value = match property {
                        "InvocationID" => Variant::new(vec![0_u8; 16]),
                        "ActiveState" => Variant::from("inactive"),
                        other => panic!("unexpected property {other}"),
                    };
                    server.reply(&header, &value).await.unwrap();
                }
                other => panic!("unexpected method {other}"),
            }
        }
        unreachable!("the selected read must return an error")
    });
    (client, worker)
}

#[tokio::test]
async fn collection_at_every_read_is_treated_as_an_already_stopped_unit() {
    for missing_at in 1..=5 {
        let (connection, server) = fixture(missing_at, false).await;
        assert!(!verify(&connection, "compositor.service", &[1; 16]).await.unwrap());
        assert_eq!(server.await.unwrap(), missing_at);
    }
}

#[tokio::test]
async fn access_denial_during_verification_is_never_treated_as_collection() {
    for denied_at in 1..=5 {
        let (connection, server) = fixture(denied_at, true).await;
        let error = verify(&connection, "compositor.service", &[1; 16]).await.unwrap_err();
        assert!(matches!(error, zbus::Error::FDO(error) if matches!(*error, zbus::fdo::Error::AccessDenied(_))));
        assert_eq!(server.await.unwrap(), denied_at);
    }
}

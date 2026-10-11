//! Select the concrete IPC transport for a recorded compositor endpoint.

use std::path::Path;

use serde::Deserialize;

use super::{SessionError, niri, sway};

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub(super) struct Window {
    pub(super) id: u64,
    pub(super) pid: Option<i32>,
}

pub(super) enum Connection { Niri(niri::Connection), Sway(sway::Connection) }
pub(super) enum Events { Niri(niri::EventStream), Sway(sway::EventStream) }

impl Connection {
    pub(super) async fn connect(name: &str, path: &Path, uid: u32) -> Result<Self, SessionError> {
        match name {
            "NIRI_SOCKET" => Ok(Self::Niri(niri::Connection::connect(path, uid).await?)),
            "SWAYSOCK" => Ok(Self::Sway(sway::Connection::connect(path, uid).await?)),
            _ => Err(SessionError::State("unsupported compositor endpoint".into())),
        }
    }

    pub(super) fn pid(&self) -> u32 {
        match self { Self::Niri(connection) => connection.pid, Self::Sway(connection) => connection.pid }
    }

    pub(super) async fn windows(&mut self) -> Result<Vec<Window>, SessionError> {
        match self { Self::Niri(connection) => connection.windows().await, Self::Sway(connection) => connection.windows().await }
    }

    pub(super) async fn close(&mut self, id: u64) -> Result<bool, SessionError> {
        match self { Self::Niri(connection) => connection.close(id).await, Self::Sway(connection) => connection.close(id).await }
    }

    pub(super) async fn subscribe(&mut self, subscriber: Self) -> Result<(Events, Vec<Window>), SessionError> {
        match (self, subscriber) {
            (Self::Niri(_), Self::Niri(subscriber)) => {
                let (events, windows) = niri::EventStream::subscribe(subscriber).await?;
                Ok((Events::Niri(events), windows))
            }
            (Self::Sway(connection), Self::Sway(subscriber)) => {
                let events = sway::EventStream::subscribe(subscriber).await?;
                Ok((Events::Sway(events), connection.windows().await?))
            }
            _ => Err(SessionError::State("compositor event transport changed".into())),
        }
    }
}

impl Events {
    pub(super) fn unchanged(&mut self, window: &Window) -> Result<bool, SessionError> {
        match self { Self::Niri(events) => events.unchanged(window), Self::Sway(events) => events.unchanged(window) }
    }
}

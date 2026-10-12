//! Select the concrete IPC transport for a recorded compositor endpoint.

use std::path::Path;

use serde::Deserialize;

use super::{SessionError, hyprland, kwin, niri, sway};

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub(super) struct Window {
    pub(super) id: u64,
    pub(super) pid: Option<i32>,
}

pub(super) enum Connection { Niri(niri::Connection), Sway(sway::Connection), Hyprland(hyprland::Connection), KWin(kwin::Connection) }
pub(super) enum Events { Niri(niri::EventStream), Sway(sway::EventStream), Hyprland(hyprland::EventStream), KWin }

impl Connection {
    pub(super) async fn connect(name: &str, path: &Path, uid: u32) -> Result<Self, SessionError> {
        match name {
            "NIRI_SOCKET" => Ok(Self::Niri(niri::Connection::connect(path, uid).await?)),
            "SWAYSOCK" => Ok(Self::Sway(sway::Connection::connect(path, uid).await?)),
            "HYPRLAND_INSTANCE_SIGNATURE" => Ok(Self::Hyprland(hyprland::Connection::connect(path, uid).await?)),
            "WAYLAND_DISPLAY" => Ok(Self::KWin(kwin::Connection::connect(path, uid).await?)),
            _ => Err(SessionError::State("unsupported compositor endpoint".into())),
        }
    }

    pub(super) fn pid(&self) -> u32 {
        match self { Self::Niri(connection) => connection.pid, Self::Sway(connection) => connection.pid, Self::Hyprland(connection) => connection.pid, Self::KWin(connection) => connection.pid }
    }

    pub(super) async fn windows(&mut self) -> Result<Vec<Window>, SessionError> {
        match self { Self::Niri(connection) => connection.windows().await, Self::Sway(connection) => connection.windows().await, Self::Hyprland(connection) => connection.windows().await, Self::KWin(connection) => connection.windows().await }
    }

    pub(super) async fn close(&mut self, id: u64) -> Result<bool, SessionError> {
        match self { Self::Niri(connection) => connection.close(id).await, Self::Sway(connection) => connection.close(id).await, Self::Hyprland(connection) => connection.close(id).await, Self::KWin(connection) => connection.close(id).await }
    }

    pub(super) fn order_windows(&self, windows: &mut [Window]) {
        if let Self::KWin(connection) = self { connection.order_windows(windows); }
        else { windows.sort_by_key(|window| window.id); }
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
            (Self::Hyprland(_), Self::Hyprland(subscriber)) => {
                let (events, windows) = hyprland::EventStream::subscribe(subscriber).await?;
                Ok((Events::Hyprland(events), windows))
            }
            _ => Err(SessionError::State("compositor event transport changed".into())),
        }
    }
}

impl Events {
    pub(super) fn unchanged(&mut self, connection: &mut Connection, window: &Window) -> Result<bool, SessionError> {
        match self {
            Self::Niri(events) => events.unchanged(window), Self::Sway(events) => events.unchanged(window), Self::Hyprland(events) => events.unchanged(window),
            Self::KWin => match connection {
                Connection::KWin(connection) => connection.unchanged(window),
                _ => Err(SessionError::State("KWin event transport changed".into())),
            },
        }
    }
}

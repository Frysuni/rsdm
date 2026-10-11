//! Veto lifecycle changes on Hyprland's separately authenticated event socket.

use tokio::net::UnixStream;

use super::{Client, Connection, SessionError, Window, compositor_socket, parse_hex};
use crate::processes::ProcessHandle;

pub(crate) struct EventStream {
    stream: UnixStream,
    peer: ProcessHandle,
    targets: Vec<(u64, u64)>,
}

impl EventStream {
    pub(crate) async fn subscribe(mut connection: Connection) -> Result<(Self, Vec<Window>), SessionError> {
        let (stream, pid, peer) = compositor_socket::connect(&connection.directory.join(".socket2.sock"), connection.uid).await?;
        if pid != connection.pid || !connection.peer.alive()? { return Err(SessionError::State("Hyprland event instance changed".into())); }
        let windows = connection.windows().await?;
        let targets = connection.clients.iter().filter_map(|client| {
            let window = Client::window(client)?;
            Some((window.id, parse_hex(client.address.strip_prefix("0x")?)?))
        }).collect();
        Ok((Self { stream, peer, targets }, windows))
    }

    pub(crate) fn unchanged(&mut self, selected: &Window) -> Result<bool, SessionError> {
        if !self.peer.alive()? { return Ok(false); }
        let Some((_, address)) = self.targets.iter().find(|(id, _)| *id == selected.id) else { return Ok(false); };
        let pending = compositor_socket::pending_events(&self.stream, Vec::new())?;
        if !pending.is_empty() && !pending.ends_with(b"\n") { return Err(SessionError::State("incomplete Hyprland event".into())); }
        let events = std::str::from_utf8(&pending).map_err(|error| SessionError::State(format!("invalid Hyprland event: {error}")))?;
        for line in events.lines() {
            let (kind, data) = line.split_once(">>").ok_or_else(|| SessionError::State("invalid Hyprland event frame".into()))?;
            if matches!(kind, "openwindow" | "closewindow") {
                let changed = data.split(',').next().and_then(parse_hex)
                    .ok_or_else(|| SessionError::State("Hyprland window event has no address".into()))?;
                if changed == *address { return Ok(false); }
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
#[path = "hyprland_events_tests.rs"]
mod tests;

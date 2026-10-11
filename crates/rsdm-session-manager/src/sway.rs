//! Direct, bounded i3-compatible Sway IPC with authenticated process identity.

use std::{path::Path, time::Duration};

use serde::Deserialize;
use serde_json::Value;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::UnixStream};

use super::{SessionError, compositor_socket::{self, MAX_FRAME_BYTES}, processes::ProcessHandle, window_backend::Window};

#[path = "sway_events.rs"]
mod events;
pub(super) use events::EventStream;

const HEADER_BYTES: usize = 14;
const MAGIC: &[u8; 6] = b"i3-ipc";

#[derive(Deserialize)]
struct Node {
    id: u64,
    pid: Option<i32>,
    shell: Option<String>,
    #[serde(default)]
    nodes: Vec<Node>,
    #[serde(default)]
    floating_nodes: Vec<Node>,
}

pub(super) struct Connection {
    stream: UnixStream,
    pub(super) pid: u32,
    peer: ProcessHandle,
}

impl Connection {
    pub(super) async fn connect(path: &Path, uid: u32) -> Result<Self, SessionError> {
        let (stream, pid, peer) = compositor_socket::connect(path, uid).await?;
        Ok(Self { stream, pid, peer })
    }

    pub(super) async fn windows(&mut self) -> Result<Vec<Window>, SessionError> {
        let tree = self.request(4, b"").await?;
        let tree: Node = serde_json::from_value(tree)
            .map_err(|error| SessionError::State(format!("invalid Sway tree: {error}")))?;
        let mut pending = vec![tree];
        let mut windows = Vec::new();
        while let Some(mut node) = pending.pop() {
            pending.append(&mut node.nodes);
            pending.append(&mut node.floating_nodes);
            if node.shell.as_deref() == Some("xdg_shell") && node.pid.is_some_and(|pid| pid > 0) {
                windows.push(Window { id: node.id, pid: node.pid });
            }
        }
        Ok(windows)
    }

    pub(super) async fn close(&mut self, id: u64) -> Result<bool, SessionError> {
        let command = format!("[con_id={id}] kill");
        let response = self.request(0, command.as_bytes()).await?;
        let results = response.as_array().filter(|results| results.len() == 1)
            .ok_or_else(|| SessionError::State("Sway close response is unconfirmed".into()))?;
        results[0].get("success").and_then(Value::as_bool)
            .ok_or_else(|| SessionError::State("Sway close response is unconfirmed".into()))
    }

    async fn request(&mut self, kind: u32, payload: &[u8]) -> Result<Value, SessionError> {
        tokio::time::timeout(Duration::from_secs(2), async {
            if !self.peer.alive()? { return Err(SessionError::State("Sway socket owner has exited".into())); }
            let mut message = MAGIC.to_vec();
            message.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
            message.extend_from_slice(&kind.to_ne_bytes());
            message.extend_from_slice(payload);
            self.stream.write_all(&message).await?;
            let mut header = [0_u8; HEADER_BYTES];
            self.stream.read_exact(&mut header).await?;
            let (length, reply) = decode_header(&header)?;
            if reply != kind { return Err(SessionError::State("unexpected Sway reply type".into())); }
            let mut payload = vec![0_u8; length];
            self.stream.read_exact(&mut payload).await?;
            serde_json::from_slice(&payload).map_err(|error| SessionError::State(format!("invalid Sway reply: {error}")))
        }).await.map_err(|_| SessionError::State("Sway IPC timed out".into()))?
    }
}

fn decode_header(header: &[u8]) -> Result<(usize, u32), SessionError> {
    if header.len() < HEADER_BYTES || &header[..6] != MAGIC {
        return Err(SessionError::State("invalid or incomplete Sway header".into()));
    }
    let length = u32::from_ne_bytes(header[6..10].try_into().expect("length bytes"));
    let kind = u32::from_ne_bytes(header[10..14].try_into().expect("type bytes"));
    if u64::from(length) > MAX_FRAME_BYTES { return Err(SessionError::State("Sway frame is oversized".into())); }
    Ok((length as usize, kind))
}

#[cfg(test)]
#[path = "sway_tests.rs"]
mod tests;

//! Read-only Wayfire discovery; its view IDs and reported PIDs cannot authorize close.

use std::{path::Path, time::Duration};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::UnixStream};

use super::{SessionError, compositor_socket::{self, MAX_FRAME_BYTES}, processes::ProcessHandle};

const LIST_VIEWS: &str = "window-rules/list-views";
const VIEW_INFO: &str = "window-rules/view-info";

#[derive(Deserialize)]
struct View { id: u32 }

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

    pub(super) async fn discover(&mut self) -> Result<(), SessionError> {
        let response = self.request("list-methods", json!({})).await?;
        let methods: Vec<String> = serde_json::from_value(response.get("methods").cloned().unwrap_or(Value::Null))
            .map_err(|error| SessionError::State(format!("invalid Wayfire method list: {error}")))?;
        for method in [LIST_VIEWS, VIEW_INFO, "window-rules/close-view"] {
            if !methods.iter().any(|name| name == method) {
                return Err(SessionError::State(format!("Wayfire ipc-rules is unavailable or incompatible: missing {method}")));
            }
        }

        let views: Vec<View> = serde_json::from_value(self.request(LIST_VIEWS, json!({})).await?)
            .map_err(|error| SessionError::State(format!("invalid Wayfire view list: {error}")))?;
        if let Some(view) = views.first() {
            let response = self.request(VIEW_INFO, json!({"id": view.id})).await?;
            let current: View = serde_json::from_value(response.get("info").cloned().unwrap_or(Value::Null))
                .map_err(|error| SessionError::State(format!("invalid Wayfire view info: {error}")))?;
            if current.id != view.id { return Err(SessionError::State("Wayfire view changed during discovery".into())); }
        }
        Ok(())
    }

    async fn request(&mut self, method: &str, data: Value) -> Result<Value, SessionError> {
        tokio::time::timeout(Duration::from_secs(2), async {
            if !self.peer.alive()? { return Err(SessionError::State("Wayfire socket owner has exited".into())); }
            let payload = serde_json::to_vec(&json!({"method": method, "data": data}))
                .map_err(|error| SessionError::State(error.to_string()))?;
            self.stream.write_all(&(payload.len() as u32).to_ne_bytes()).await?;
            self.stream.write_all(&payload).await?;
            let response = self.read_frame().await?;
            if let Some(error) = response.get("error") {
                return Err(SessionError::State(format!("Wayfire {method}: {error}")));
            }
            if !self.peer.alive()? { return Err(SessionError::State("Wayfire socket owner has exited".into())); }
            Ok(response)
        }).await.map_err(|_| SessionError::State("Wayfire IPC timed out".into()))?
    }

    async fn read_frame(&mut self) -> Result<Value, SessionError> {
        let mut header = [0_u8; 4];
        self.stream.read_exact(&mut header).await?;
        let length = u32::from_ne_bytes(header);
        if length == 0 || u64::from(length) > MAX_FRAME_BYTES {
            return Err(SessionError::State("empty or oversized Wayfire frame".into()));
        }
        let mut frame = vec![0_u8; length as usize];
        self.stream.read_exact(&mut frame).await?;
        serde_json::from_slice(&frame).map_err(|error| SessionError::State(format!("invalid Wayfire frame: {error}")))
    }
}

#[cfg(test)]
#[path = "wayfire_tests.rs"]
pub(crate) mod tests;

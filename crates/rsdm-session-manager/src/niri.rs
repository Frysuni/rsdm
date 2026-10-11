//! Niri's bounded, newline-delimited JSON transport on one pinned connection.

use std::{path::Path, time::Duration};

use serde_json::{Value, json};
use tokio::{io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader}, net::UnixStream};

use super::{SessionError, compositor_socket::{self, MAX_FRAME_BYTES}, processes::ProcessHandle};
pub(super) use super::window_backend::Window;

#[path = "niri_events.rs"]
mod events;
pub(super) use events::EventStream;

pub(super) struct Connection {
    stream: BufReader<UnixStream>,
    pub(super) pid: u32,
    peer: ProcessHandle,
}

impl Connection {
    pub(super) async fn connect(path: &Path, uid: u32) -> Result<Self, SessionError> {
        let (stream, pid, peer) = compositor_socket::connect(path, uid).await?;
        Ok(Self { stream: BufReader::new(stream), pid, peer })
    }

    pub(super) async fn windows(&mut self) -> Result<Vec<Window>, SessionError> {
        let response = self.request(&json!("Windows")).await?;
        let windows = response.get("Ok").and_then(|response| response.get("Windows"))
            .ok_or_else(|| SessionError::State("Niri did not return a window snapshot".into()))?;
        serde_json::from_value(windows.clone()).map_err(|error| SessionError::State(format!("invalid Niri windows: {error}")))
    }

    pub(super) async fn close(&mut self, id: u64) -> Result<bool, SessionError> {
        let response = self.request(&json!({"Action": {"CloseWindow": {"id": id}}})).await?;
        if response.get("Ok") == Some(&json!("Handled")) { return Ok(true); }
        // Niri validates actions before scheduling them. A protocol error is
        // a rejection; any other response remains an unconfirmed dispatch.
        if response.get("Err").is_some_and(Value::is_string) { return Ok(false); }
        Err(SessionError::State("Niri close response is unconfirmed".into()))
    }

    async fn request(&mut self, request: &Value) -> Result<Value, SessionError> {
        tokio::time::timeout(Duration::from_secs(2), async {
            if !self.peer.alive()? { return Err(SessionError::State("Niri socket owner has exited".into())); }
            let mut request = serde_json::to_vec(request).map_err(|error| SessionError::State(error.to_string()))?;
            request.push(b'\n');
            self.stream.get_mut().write_all(&request).await?;
            self.read_frame().await
        }).await.map_err(|_| SessionError::State("Niri IPC timed out".into()))?
    }

    async fn read_frame(&mut self) -> Result<Value, SessionError> {
        let mut frame = Vec::new();
        (&mut self.stream).take(MAX_FRAME_BYTES + 1).read_until(b'\n', &mut frame).await?;
        if frame.len() as u64 > MAX_FRAME_BYTES || frame.last() != Some(&b'\n') {
            return Err(SessionError::State("incomplete or oversized Niri frame".into()));
        }
        serde_json::from_slice(&frame).map_err(|error| SessionError::State(format!("invalid Niri frame: {error}")))
    }
}

#[cfg(test)]
#[path = "niri_tests.rs"]
mod tests;

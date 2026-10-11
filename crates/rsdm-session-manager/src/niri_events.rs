//! Observe the selected window without a background task or another close.

use std::{io::ErrorKind, os::fd::AsRawFd};

use super::{Connection, Duration, SessionError, Value, Window, MAX_FRAME_BYTES, json};
use tokio::io::AsyncBufRead;

pub(crate) struct EventStream {
    connection: Connection,
}

impl EventStream {
    pub(crate) async fn subscribe(mut connection: Connection) -> Result<(Self, Vec<Window>), SessionError> {
        let response = connection.request(&json!("EventStream")).await?;
        if response.get("Ok") != Some(&json!("Handled")) {
            return Err(SessionError::State("Niri event subscription was rejected".into()));
        }
        let windows = tokio::time::timeout(Duration::from_secs(2), async {
            // The initial burst includes unrelated workspace and keyboard events.
            for _ in 0..256 {
                let event = connection.read_frame().await?;
                if let Some(windows) = event.get("WindowsChanged").and_then(|event| event.get("windows")) {
                    return serde_json::from_value(windows.clone())
                        .map_err(|error| SessionError::State(format!("invalid Niri window event: {error}")));
                }
            }
            Err(SessionError::State("Niri did not publish its initial windows".into()))
        }).await.map_err(|_| SessionError::State("Niri event snapshot timed out".into()))??;
        Ok((Self { connection }, windows))
    }

    pub(crate) fn unchanged(&mut self, window: &Window) -> Result<bool, SessionError> {
        if !self.connection.peer.alive()? { return Ok(false); }
        let events = self.pending_events()?;
        let mut unchanged = true;
        for frame in events.split_inclusive(|byte| *byte == b'\n') {
            if frame.last() != Some(&b'\n') {
                return Err(SessionError::State("Niri event is incomplete before close".into()));
            }
            let event = serde_json::from_slice(frame)
                .map_err(|error| SessionError::State(format!("invalid Niri event: {error}")))?;
            unchanged &= preserves_window(event, window)?;
        }
        Ok(unchanged)
    }

    fn pending_events(&mut self) -> Result<Vec<u8>, SessionError> {
        let buffered = self.connection.stream.buffer();
        let mut events = buffered.to_vec();
        let consumed = buffered.len();
        std::pin::Pin::new(&mut self.connection.stream).consume(consumed);
        let mut buffer = [0_u8; 4096];
        loop {
            // Read kernel-queued events too, even if Tokio has not yet polled
            // readiness since the command connection's last snapshot.
            // SAFETY: the socket FD and mutable buffer are valid for this call.
            let count = unsafe { libc::recv(self.connection.stream.get_ref().as_raw_fd(),
                buffer.as_mut_ptr().cast(), buffer.len(), libc::MSG_DONTWAIT) };
            let read = if count < 0 { Err(std::io::Error::last_os_error()) } else { Ok(count as usize) };
            match read {
                Ok(0) => return Err(SessionError::State("Niri event stream disconnected before close".into())),
                Ok(count) => events.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(events),
                Err(error) => return Err(error.into()),
            }
            if events.len() as u64 > MAX_FRAME_BYTES {
                return Err(SessionError::State("Niri event burst is oversized".into()));
            }
        }
    }
}

fn preserves_window(event: Value, selected: &Window) -> Result<bool, SessionError> {
    if !event.is_object() { return Err(SessionError::State("invalid Niri event object".into())); }
    let event_window = |value: Value| serde_json::from_value::<Window>(value)
        .map_err(|error| SessionError::State(format!("invalid Niri window event: {error}")));
    if let Some(event) = event.get("WindowsChanged") {
        let windows: Vec<Window> = serde_json::from_value(event.get("windows").cloned().unwrap_or(Value::Null))
            .map_err(|error| SessionError::State(format!("invalid Niri window snapshot: {error}")))?;
        return Ok(windows.contains(selected));
    }
    if let Some(event) = event.get("WindowOpenedOrChanged") {
        let window = event_window(event.get("window").cloned().unwrap_or(Value::Null))?;
        return Ok(window.id != selected.id || window == *selected);
    }
    if let Some(event) = event.get("WindowClosed") {
        let id = event.get("id").and_then(Value::as_u64)
            .ok_or_else(|| SessionError::State("invalid Niri closed window identity".into()))?;
        return Ok(id != selected.id);
    }
    Ok(true)
}

#[cfg(test)]
#[path = "niri_events_tests.rs"]
mod tests;

//! Observe the selected window without a background task or another close.

use super::{Connection, Duration, SessionError, Value, Window, json};
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
        let events = buffered.to_vec();
        let consumed = buffered.len();
        std::pin::Pin::new(&mut self.connection.stream).consume(consumed);
        super::compositor_socket::pending_events(self.connection.stream.get_ref(), events)
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

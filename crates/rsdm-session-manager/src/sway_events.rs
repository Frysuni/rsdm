//! Reject stale Sway containers using a separate subscribed connection.

use super::{Connection, HEADER_BYTES, Node, SessionError, Value, Window, compositor_socket, decode_header};

pub(crate) struct EventStream { connection: Connection }

impl EventStream {
    pub(crate) async fn subscribe(mut connection: Connection) -> Result<Self, SessionError> {
        let response = connection.request(2, br#"["window"]"#).await?;
        if response.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(SessionError::State("Sway event subscription was rejected".into()));
        }
        Ok(Self { connection })
    }

    pub(crate) fn unchanged(&mut self, selected: &Window) -> Result<bool, SessionError> {
        if !self.connection.peer.alive()? { return Ok(false); }
        let pending = compositor_socket::pending_events(&self.connection.stream, Vec::new())?;
        let mut frames = pending.as_slice();
        let mut unchanged = true;
        while !frames.is_empty() {
            let (length, kind) = decode_header(frames)?;
            if kind != 0x80000003 || frames.len() < HEADER_BYTES + length {
                return Err(SessionError::State("unexpected or incomplete Sway event".into()));
            }
            let event = serde_json::from_slice(&frames[HEADER_BYTES..HEADER_BYTES + length])
                .map_err(|error| SessionError::State(format!("invalid Sway event: {error}")))?;
            unchanged &= preserves_window(event, selected)?;
            frames = &frames[HEADER_BYTES + length..];
        }
        Ok(unchanged)
    }
}

fn preserves_window(event: Value, selected: &Window) -> Result<bool, SessionError> {
    let change = event.get("change").and_then(Value::as_str)
        .ok_or_else(|| SessionError::State("Sway window event has no change type".into()))?;
    let container: Node = serde_json::from_value(event.get("container").cloned().unwrap_or(Value::Null))
        .map_err(|error| SessionError::State(format!("invalid Sway window event: {error}")))?;
    if container.id != selected.id { return Ok(true); }
    Ok(!matches!(change, "new" | "close") && container.pid == selected.pid && container.shell.as_deref() == Some("xdg_shell"))
}

#[cfg(test)]
#[path = "sway_events_tests.rs"]
mod tests;

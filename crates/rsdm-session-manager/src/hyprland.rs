//! Authenticate Hyprland IPC and close only after an atomic stable-ID check.

use std::{path::{Path, PathBuf}, time::Duration};

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{SessionError, compositor_socket::{self, MAX_FRAME_BYTES}, processes::ProcessHandle, window_backend::Window};

#[path = "hyprland_events.rs"]
mod events;
pub(super) use events::EventStream;

const PROBE: &str = "/eval assert(type(hl.get_windows)=='function' and type(hl.dsp.window.close)=='function' and type(math.type)=='function')";
const TARGET_CHANGED: &str = "rsdm: window-close target changed";

#[derive(Deserialize)]
struct Client {
    address: String,
    #[serde(rename = "stableId")]
    stable_id: Option<String>,
    pid: Option<i32>,
    mapped: bool,
    xwayland: bool,
}

impl Client {
    fn window(&self) -> Option<Window> {
        if !self.mapped || self.xwayland { return None; }
        let pid = self.pid.filter(|pid| *pid > 0)?;
        let id = parse_hex(self.stable_id.as_deref()?)?;
        parse_hex(self.address.strip_prefix("0x")?)?;
        Some(Window { id, pid: Some(pid) })
    }
}

pub(super) struct Connection {
    directory: PathBuf,
    uid: u32,
    pub(super) pid: u32,
    peer: ProcessHandle,
    clients: Vec<Client>,
}

impl Connection {
    pub(super) async fn connect(directory: &Path, uid: u32) -> Result<Self, SessionError> {
        let (_stream, pid, peer) = compositor_socket::connect(&directory.join(".socket.sock"), uid).await?;
        Ok(Self { directory: directory.into(), uid, pid, peer, clients: Vec::new() })
    }

    pub(super) async fn windows(&mut self) -> Result<Vec<Window>, SessionError> {
        if self.request(PROBE).await? != "ok" {
            return Err(SessionError::State("Hyprland atomic stable-ID close requires the Lua API".into()));
        }
        self.clients = serde_json::from_str(&self.request("j/clients").await?)
            .map_err(|error| SessionError::State(format!("invalid Hyprland clients: {error}")))?;
        Ok(self.clients.iter().filter_map(Client::window).collect())
    }

    pub(super) async fn close(&mut self, id: u64) -> Result<bool, SessionError> {
        let Some(client) = self.clients.iter().find(|client| client.window().is_some_and(|window| window.id == id)) else { return Ok(false); };
        // The compositor runs this check and dispatcher in the same Lua call.
        // Numeric fields are validated before interpolation; titles and app IDs
        // never select a target. Address reuse cannot substitute another ID.
        let address = parse_hex(client.address.strip_prefix("0x").unwrap_or("")).expect("validated client address");
        // Lua exposed an integer in 0.55 and now exposes a hexadecimal string.
        // Restrict numeric comparison to integers to avoid rounding large IDs.
        let identity = format!("(math.type(w.stable_id)=='integer' and w.stable_id==0x{id:x}) or w.stable_id=='{id:x}'");
        let command = format!("/eval for _,w in ipairs(hl.get_windows()) do if ({identity}) and w.address=='0x{address:x}' and w.pid=={} and w.mapped and not w.xwayland then hl.dispatch(hl.dsp.window.close({{window='address:0x{address:x}'}})); return end end error('{TARGET_CHANGED}',0)", client.pid.expect("validated client PID"));
        match self.request(&command).await?.as_str() {
            "ok" => Ok(true),
            response if response == format!("error: {TARGET_CHANGED}") => Ok(false),
            _ => Err(SessionError::State("Hyprland close response is unconfirmed".into())),
        }
    }

    async fn request(&mut self, command: &str) -> Result<String, SessionError> {
        tokio::time::timeout(Duration::from_secs(2), async {
            if !self.peer.alive()? { return Err(SessionError::State("Hyprland socket owner has exited".into())); }
            let (mut stream, pid, _) = compositor_socket::connect(&self.directory.join(".socket.sock"), self.uid).await?;
            if pid != self.pid || !self.peer.alive()? { return Err(SessionError::State("Hyprland IPC instance changed".into())); }
            stream.write_all(command.as_bytes()).await?;
            stream.shutdown().await?;
            let mut response = Vec::new();
            stream.take(MAX_FRAME_BYTES + 1).read_to_end(&mut response).await?;
            if response.len() as u64 > MAX_FRAME_BYTES { return Err(SessionError::State("Hyprland response is oversized".into())); }
            String::from_utf8(response).map_err(|error| SessionError::State(format!("invalid Hyprland response: {error}")))
        }).await.map_err(|_| SessionError::State("Hyprland IPC timed out".into()))?
    }
}

pub(super) fn socket_directory(environment: &[(String, String)], signature: &str) -> Option<PathBuf> {
    if signature.is_empty() || !signature.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) { return None; }
    let (_, directory) = environment.iter().find(|(name, _)| name == "XDG_RUNTIME_DIR")?;
    let directory = Path::new(directory);
    directory.is_absolute().then(|| directory.join("hypr").join(signature))
}

fn parse_hex(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) { return None; }
    u64::from_str_radix(value, 16).ok().filter(|value| *value != 0)
}

#[cfg(test)]
#[path = "hyprland_tests.rs"]
mod tests;

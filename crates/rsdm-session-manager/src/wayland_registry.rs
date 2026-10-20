//! Read-only core registry discovery; no desktop or window-manager role is bound.

use std::{collections::BTreeMap, path::{Component, Path, PathBuf}, time::Duration};

use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::UnixStream};

use super::{SessionError, compositor_socket, processes::ProcessHandle};

const INTERFACES: &[&str] = &[
    "ext_foreign_toplevel_list_v1",
    "zwlr_foreign_toplevel_manager_v1",
    "org_kde_plasma_window_management",
];

pub(super) struct Connection {
    pub(super) stream: UnixStream,
    pub(super) pid: u32,
    pub(super) peer: ProcessHandle,
}

impl Connection {
    pub(super) async fn connect(path: &Path, uid: u32) -> Result<Self, SessionError> {
        let (stream, pid, peer) = compositor_socket::connect(path, uid).await?;
        Ok(Self { stream, pid, peer })
    }

    pub(super) async fn discover(&mut self) -> Result<Vec<String>, SessionError> {
        let globals = self.globals().await?;
        Ok(INTERFACES.iter().filter_map(|interface| {
            let version = globals.values().filter(|(name, _)| name == interface).map(|(_, version)| *version).max()?;
            Some(format!("{interface} v{version}"))
        }).collect())
    }

    pub(super) async fn globals(&mut self) -> Result<BTreeMap<u32, (String, u32)>, SessionError> {
        tokio::time::timeout(Duration::from_secs(2), self.registry()).await
            .map_err(|_| SessionError::State("Wayland registry discovery timed out".into()))?
    }

    async fn registry(&mut self) -> Result<BTreeMap<u32, (String, u32)>, SessionError> {
        if !self.peer.alive()? { return Err(invalid("socket owner has exited")); }
        // wl_display.get_registry(new_id=2), then wl_display.sync(new_id=3).
        for words in [[1_u32, (12 << 16) | 1, 2], [1, 12 << 16, 3]] {
            for word in words { self.stream.write_all(&word.to_ne_bytes()).await?; }
        }
        let mut globals = BTreeMap::new();
        let mut received = 0_u64;
        loop {
            let (object, opcode, body) = self.event(&mut received).await?;
            if apply_event(&mut globals, object, opcode, &body)? { break; }
        }
        if !self.peer.alive()? { return Err(invalid("socket owner exited during discovery")); }
        Ok(globals)
    }

    pub(super) async fn request(&mut self, object: u32, opcode: u16, body: &[u8]) -> Result<(), SessionError> {
        if !self.peer.alive()? { return Err(invalid("socket owner has exited")); }
        let size = body.len() + 8;
        if size > u16::MAX as usize || size % 4 != 0 { return Err(invalid("invalid request size")); }
        let mut frame = object.to_ne_bytes().to_vec();
        frame.extend_from_slice(&(((size as u32) << 16) | u32::from(opcode)).to_ne_bytes());
        frame.extend_from_slice(body);
        self.stream.write_all(&frame).await?;
        Ok(())
    }

    pub(super) async fn event(&mut self, received: &mut u64) -> Result<(u32, u16, Vec<u8>), SessionError> {
        let mut header = [0_u8; 8];
        self.stream.read_exact(&mut header).await?;
        let object = u32::from_ne_bytes(header[..4].try_into().expect("four-byte word"));
        let word = u32::from_ne_bytes(header[4..].try_into().expect("four-byte word"));
        let size = word >> 16;
        if size < 8 || size % 4 != 0 { return Err(invalid("invalid event size")); }
        *received += u64::from(size);
        if *received > compositor_socket::MAX_FRAME_BYTES { return Err(invalid("registry event burst is oversized")); }
        let mut body = vec![0; size as usize - 8];
        self.stream.read_exact(&mut body).await?;
        Ok((object, word as u16, body))
    }
}

pub(super) fn socket_path(environment: &[(String, String)]) -> Option<PathBuf> {
    let value = |name: &str| environment.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
    let display = Path::new(value("WAYLAND_DISPLAY")?);
    if display.as_os_str().is_empty() || display.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir)) {
        return None;
    }
    if display.is_absolute() { return Some(display.into()); }
    let runtime = Path::new(value("XDG_RUNTIME_DIR")?);
    if !runtime.is_absolute() { return None; }
    Some(runtime.join(display))
}

fn apply_event(globals: &mut BTreeMap<u32, (String, u32)>, object: u32, opcode: u16, body: &[u8]) -> Result<bool, SessionError> {
    match (object, opcode) {
        (2, 0) => {
            let (id, interface, version) = global(body)?;
            if globals.insert(id, (interface, version)).is_some() { return Err(invalid("duplicate registry global")); }
        }
        (2, 1) if body.len() == 4 => { globals.remove(&word(body, 0)?); }
        (3, 0) if body.len() == 4 => return Ok(true),
        (1, 0) => return Err(invalid("compositor reported a protocol error")),
        _ => return Err(invalid("unexpected registry event")),
    }
    Ok(false)
}

pub(super) fn global(body: &[u8]) -> Result<(u32, String, u32), SessionError> {
    let id = word(body, 0)?;
    let length = word(body, 4)? as usize;
    if id == 0 || length < 2 || length > body.len().saturating_sub(12) { return Err(invalid("invalid registry global")); }
    let end = 8 + length;
    let padded = (end + 3) & !3;
    if body.len() != padded + 4 || body[end - 1] != 0 || body[8..end - 1].contains(&0) {
        return Err(invalid("invalid registry interface string"));
    }
    let interface = std::str::from_utf8(&body[8..end - 1]).map_err(|_| invalid("invalid registry interface encoding"))?;
    let version = word(body, padded)?;
    if version == 0 { return Err(invalid("invalid registry interface version")); }
    Ok((id, interface.into(), version))
}

fn word(body: &[u8], offset: usize) -> Result<u32, SessionError> {
    let bytes = body.get(offset..offset + 4).ok_or_else(|| invalid("incomplete registry event"))?;
    Ok(u32::from_ne_bytes(bytes.try_into().expect("four-byte word")))
}

fn invalid(reason: &str) -> SessionError { SessionError::State(format!("Wayland: {reason}")) }

#[cfg(test)]
#[path = "wayland_registry_tests.rs"]
pub(crate) mod tests;

//! Track live Plasma window resources without trusting titles or application IDs.

use std::collections::{BTreeMap, BTreeSet};

use super::{SessionError, Window};

#[derive(Default)]
struct Resource { pid: Option<i32>, parent: u32, initialized: bool, unmapped: bool, closeable: bool }

#[derive(Default)]
pub(super) struct State {
    resources: BTreeMap<u32, Resource>,
    uuids: BTreeSet<String>,
    pub(super) pending: Vec<String>,
}

impl State {
    pub(super) fn insert(&mut self, id: u32) -> Result<(), SessionError> {
        if self.resources.insert(id, Resource::default()).is_some() { return Err(invalid("duplicate window resource")); }
        Ok(())
    }

    pub(super) fn event(&mut self, object: u32, opcode: u16, body: &[u8]) -> Result<(), SessionError> {
        match (object, opcode) {
            (1, 1) if body.len() == 4 => return Ok(()),
            (2, 1) if body.len() == 4 => return Err(invalid("window-management global was removed")),
            (4, 4) => {
                let uuid = parse_string(body.get(4..).ok_or_else(|| invalid("incomplete window announcement"))?)?;
                if self.uuids.len() >= 4096 || !self.uuids.insert(uuid.clone()) { return Err(invalid("duplicate or excessive window UUIDs")); }
                self.pending.push(uuid);
                return Ok(());
            }
            (4, 0 | 1 | 2 | 3 | 5) => return Ok(()),
            _ => {}
        }
        let resource = self.resources.get_mut(&object).ok_or_else(|| invalid("unexpected protocol object"))?;
        match opcode {
            2 => resource.closeable = word(body)? & 0x100 != 0,
            5 if body.is_empty() => resource.unmapped = true,
            6 if body.is_empty() => resource.initialized = true,
            7 => resource.parent = word(body)?,
            10 => resource.pid = i32::try_from(word(body)?).ok().filter(|pid| *pid > 0),
            0 | 1 | 3 | 4 | 8 | 9 | 11..=16 => {},
            _ => return Err(invalid("unexpected window event")),
        }
        Ok(())
    }

    pub(super) fn windows(&self) -> Vec<Window> {
        self.resources.iter().filter_map(|(id, resource)| {
            if !resource.initialized || resource.unmapped || !resource.closeable { return None; }
            Some(Window { id: u64::from(*id), pid: Some(resource.pid?) })
        }).collect()
    }

    pub(super) fn has_parent(&self, id: u64) -> bool {
        self.resources.get(&(id as u32)).is_some_and(|resource| resource.parent != 0)
    }

    pub(super) fn pending_events(&mut self, frames: &[u8]) -> Result<(), SessionError> {
        let mut remaining = frames;
        while !remaining.is_empty() {
            if remaining.len() < 8 { return Err(invalid("incomplete queued event header")); }
            let object = u32::from_ne_bytes(remaining[..4].try_into().expect("four-byte word"));
            let header = u32::from_ne_bytes(remaining[4..8].try_into().expect("four-byte word"));
            let size = (header >> 16) as usize;
            if size < 8 || size % 4 != 0 || size > remaining.len() { return Err(invalid("incomplete queued event")); }
            self.event(object, header as u16, &remaining[8..size])?;
            remaining = &remaining[size..];
        }
        Ok(())
    }
}

fn word(body: &[u8]) -> Result<u32, SessionError> {
    Ok(u32::from_ne_bytes(body.try_into().map_err(|_| invalid("invalid window event word"))?))
}

fn parse_string(body: &[u8]) -> Result<String, SessionError> {
    let length = word(body.get(..4).ok_or_else(|| invalid("incomplete UUID string"))?)? as usize;
    let end = 4 + length;
    if length < 2 || length > 128 || body.len() != ((end + 3) & !3) || body.get(end - 1) != Some(&0) {
        return Err(invalid("invalid window UUID string"));
    }
    let value = &body[4..end - 1];
    if value.contains(&0) { return Err(invalid("embedded null in window UUID")); }
    std::str::from_utf8(value).map(String::from).map_err(|_| invalid("invalid UUID encoding"))
}

pub(super) fn string(value: &str) -> Vec<u8> {
    let mut body = (value.len() as u32 + 1).to_ne_bytes().to_vec();
    body.extend(value.as_bytes());
    body.push(0);
    while body.len() % 4 != 0 { body.push(0); }
    body
}

pub(super) fn invalid(reason: &str) -> SessionError { SessionError::State(format!("KWin: {reason}")) }

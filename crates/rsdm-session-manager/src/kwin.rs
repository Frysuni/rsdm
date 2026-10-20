//! KWin windows stay bound to unreused resources on one authenticated connection.

use std::{path::Path, time::Duration};

use super::{SessionError, bus::UserManager, compositor_socket, native_delegate::pin_peer,
    wayland_registry, window_backend::Window};

#[path = "kwin_state.rs"]
mod state;
use state::{State, invalid, string};

const INTERFACE: &str = "org_kde_plasma_window_management";
const MANAGEMENT: u32 = 4;

pub(super) struct Connection {
    wire: wayland_registry::Connection,
    state: State,
    next_id: u32,
    initialized: bool,
    pub(super) pid: u32,
}

impl Connection {
    pub(super) async fn connect(path: &Path, uid: u32) -> Result<Self, SessionError> {
        let wire = wayland_registry::Connection::connect(path, uid).await?;
        let pid = wire.pid;
        Ok(Self { wire, state: State::default(), next_id: 5, initialized: false, pid })
    }

    pub(super) async fn authenticate(&self, manager: &UserManager) -> Result<(), SessionError> {
        manager.request(async {
            let bus = manager.connection();
            let peer = pin_peer(&bus, "org.kde.KWin").await?;
            peer.verify_process(&bus, self.pid).await?;
            let proxy = zbus::Proxy::new(&bus, peer.owner.as_str(), "/KWin", "org.kde.KWin").await?;
            let information: String = proxy.call_with_flags("supportInformation", zbus::proxy::MethodFlags::NO_AUTO_START, &()).await?
                .ok_or_else(|| zbus::fdo::Error::Failed("KWin version reply is absent".into()))?;
            if !trusted_version(&information) {
                return Err(zbus::fdo::Error::NotSupported("KWin 6.4 or later is required for XRes-backed window PIDs".into()).into());
            }
            peer.verify_process(&bus, self.pid).await
        }).await.map_err(Into::into)
    }

    pub(super) async fn windows(&mut self) -> Result<Vec<Window>, SessionError> {
        tokio::time::timeout(Duration::from_secs(2), self.snapshot()).await
            .map_err(|_| invalid("window snapshot timed out"))?
    }

    async fn snapshot(&mut self) -> Result<Vec<Window>, SessionError> {
        if !self.initialized {
            let globals = self.wire.globals().await?;
            let (name, _) = globals.iter().find(|(_, (interface, version))| interface == INTERFACE && *version >= 17)
                .ok_or_else(|| invalid("window-management v17 is not advertised"))?;
            let mut body = name.to_ne_bytes().to_vec();
            body.extend(string(INTERFACE));
            body.extend(17_u32.to_ne_bytes());
            body.extend(MANAGEMENT.to_ne_bytes());
            self.wire.request(2, 0, &body).await?;
            self.initialized = true;
        }
        let mut received = 0;
        self.roundtrip(&mut received).await?;
        while !self.state.pending.is_empty() {
            for uuid in std::mem::take(&mut self.state.pending) {
                let id = self.allocate()?;
                self.state.insert(id)?;
                let mut body = id.to_ne_bytes().to_vec();
                body.extend(string(&uuid));
                self.wire.request(MANAGEMENT, 2, &body).await?;
            }
            self.roundtrip(&mut received).await?;
        }
        Ok(self.state.windows())
    }

    async fn roundtrip(&mut self, received: &mut u64) -> Result<(), SessionError> {
        let callback = self.allocate()?;
        self.wire.request(1, 0, &callback.to_ne_bytes()).await?;
        loop {
            let (object, opcode, body) = self.wire.event(received).await?;
            if object == callback && opcode == 0 && body.len() == 4 { break; }
            self.state.event(object, opcode, &body)?;
        }
        if !self.wire.peer.alive()? { return Err(invalid("compositor exited during snapshot")); }
        Ok(())
    }

    fn allocate(&mut self) -> Result<u32, SessionError> {
        let id = self.next_id;
        // IDs are never reused, even after wl_display.delete_id.
        self.next_id = id.checked_add(1).filter(|next| *next < 0xff00_0000)
            .ok_or_else(|| invalid("protocol object IDs are exhausted"))?;
        Ok(id)
    }

    pub(super) fn order_windows(&self, windows: &mut [Window]) {
        windows.sort_by_key(|window| (self.state.has_parent(window.id), window.id));
    }

    pub(super) fn unchanged(&mut self, window: &Window) -> Result<bool, SessionError> {
        let frames = compositor_socket::pending_events(&self.wire.stream, Vec::new())?;
        self.state.pending_events(&frames)?;
        Ok(self.wire.peer.alive()? && self.state.windows().contains(window))
    }

    pub(super) async fn close(&mut self, id: u64) -> Result<bool, SessionError> {
        let Some(window) = self.state.windows().into_iter().find(|window| window.id == id) else { return Ok(false); };
        if !self.unchanged(&window)? { return Ok(false); }
        tokio::time::timeout(Duration::from_secs(2), async {
            self.wire.request(id as u32, 4, &[]).await?;
            self.roundtrip(&mut 0).await?;
            Ok(true)
        }).await.map_err(|_| invalid("close acknowledgement timed out"))?
    }
}

fn trusted_version(information: &str) -> bool {
    let Some(version) = information.lines().find_map(|line| line.strip_prefix("KWin version: ")) else { return false; };
    let mut numbers = version.split('.');
    numbers.next() == Some("6") && numbers.next().and_then(|minor| minor.parse::<u32>().ok()).is_some_and(|minor| minor >= 4)
}

#[cfg(test)]
#[path = "kwin_tests.rs"]
mod tests;

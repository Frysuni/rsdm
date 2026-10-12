//! Socket credentials, pinned process identity and registered app ownership.

use std::{cell::Cell, os::fd::OwnedFd, ptr, rc::Rc, sync::Arc, time::Instant};

use super::{ffi, interrupt::Interrupt, properties::Properties, protocol::Phase};
use crate::{SessionError, apps, bus::UserManager,
    processes::{ProcessHandle, app_processes}, runtime::Runtime};

pub(super) struct Peer {
    pub ice: ffi::IceConn,
    pub sms: ffi::SmsConn,
    pub unit: String,
    pub invocation: Vec<u8>,
    pub pid: u32,
    process: ProcessHandle,
    _interrupt: Arc<OwnedFd>,
    pub phase: Phase,
    pub selected: bool,
    pub die_claimed: Rc<Cell<bool>>,
    pub interaction_requested: bool,
    pub interacting: bool,
    pub cancellation_requested: bool,
    pub local_save_requested: bool,
    pub deadline: u64,
    pub cancel_on_timeout: bool,
    pub properties: Properties,
    pub accepted: Instant,
    pub actions: crate::action_gate::ActionGate,
}

impl Peer {
    #[cfg(test)]
    pub(super) fn disabled_for_test() -> Box<Self> {
        use std::os::fd::AsRawFd;

        let (reader, _writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let interrupt = Interrupt::default().register(reader.as_raw_fd()).unwrap();
        let actions = crate::action_gate::ActionGate::default();
        actions.close();
        let pid = std::process::id();
        Box::new(Self {
            ice: ptr::null_mut(), sms: ptr::null_mut(), unit: String::new(), invocation: Vec::new(),
            pid, process: ProcessHandle::open(pid).unwrap().unwrap(), _interrupt: interrupt,
            phase: Phase::InitialSave, selected: false, die_claimed: Rc::default(), interaction_requested: false,
            interacting: false, cancellation_requested: false, local_save_requested: false,
            deadline: 0, cancel_on_timeout: false, properties: Properties::default(),
            accepted: Instant::now(), actions,
        })
    }

    pub async fn accept(listener: ffi::ListenObj, manager: &UserManager, runtime: &Runtime, interrupt: &Interrupt) -> Result<Box<Self>, SessionError> {
        let mut status = 0;
        // SAFETY: the listener belongs to the server; a successful accept gives
        // this function one owned connection reference.
        let ice = unsafe { ffi::IceAcceptConnection(listener, &mut status) };
        if ice.is_null() { return Err(SessionError::State("ICE connection could not be accepted".into())); }
        let mut accepted = Accepted(ice);
        unsafe { ffi::IceSetShutdownNegotiation(ice, 0); }
        let socket = interrupt.register(unsafe { ffi::IceConnectionNumber(ice) })?;
        let peer = Self::admit(ice, socket, manager, runtime).await?;
        accepted.0 = ptr::null_mut();
        Ok(peer)
    }

    async fn admit(ice: ffi::IceConn, interrupt: Arc<OwnedFd>, manager: &UserManager, runtime: &Runtime) -> Result<Box<Self>, SessionError> {
        let credentials = credentials(unsafe { ffi::IceConnectionNumber(ice) })?;
        // SAFETY: geteuid has no preconditions.
        if credentials.uid != unsafe { libc::geteuid() } || credentials.pid <= 0 {
            return Err(SessionError::State("ICE peer belongs to another user".into()));
        }
        let pid = credentials.pid as u32;
        let process = ProcessHandle::socket_peer(interrupt.as_ref())?;
        let unit = manager.unit_for_pid(pid).await?;
        let records = runtime.clone();
        let app_unit = unit.clone();
        let mut app = crate::runtime::blocking(move || records.app(&app_unit)).await?;
        if app.invocation_id.is_empty() {
            apps::pin_pending(manager, runtime, &unit).await?;
            let records = runtime.clone();
            let app_unit = unit.clone();
            app = crate::runtime::blocking(move || records.app(&app_unit)).await?;
        }
        if !process.alive()? || !app_processes(manager, &app).await?.contains(&pid) {
            return Err(SessionError::State("ICE peer is not a registered application process".into()));
        }
        Ok(Box::new(Self {
            ice, sms: ptr::null_mut(), unit, invocation: app.invocation_id, pid, process, _interrupt: interrupt,
            phase: Phase::Registering, selected: false, die_claimed: Rc::default(), interaction_requested: false, interacting: false,
            cancellation_requested: false, local_save_requested: false, deadline: 0,
            cancel_on_timeout: false, properties: Properties::default(), accepted: Instant::now(),
            actions: manager.actions.clone(),
        }))
    }

    pub async fn verified(&self, manager: &UserManager, runtime: &Runtime) -> Result<bool, SessionError> {
        if !self.alive()? { return Ok(false); }
        let records = runtime.clone();
        let unit = self.unit.clone();
        let app = crate::runtime::blocking(move || records.app(&unit)).await?;
        Ok(app.invocation_id == self.invocation && app_processes(manager, &app).await?.contains(&self.pid))
    }

    pub(super) fn alive(&self) -> Result<bool, SessionError> {
        Ok(self.phase != Phase::Closed && !self.sms.is_null() && self.process.alive()?)
    }

    pub fn fd(&self) -> i32 {
        // SAFETY: this method is used only while the owned connection is live.
        unsafe { ffi::IceConnectionNumber(self.ice) }
    }

    pub unsafe fn process(peer: *mut Self) {
        // Keep no Rust borrow of Peer across libICE's reentrant callbacks.
        let ice = unsafe { (*peer).ice };
        let result = unsafe { ffi::IceProcessMessages(ice, ptr::null_mut(), ptr::null_mut()) };
        let peer = unsafe { &mut *peer };
        if result == 2 {
            // libICE already freed the connection after protocol negotiation.
            peer.ice = ptr::null_mut();
            peer.sms = ptr::null_mut();
            peer.phase = Phase::Closed;
        } else if result != 0 { peer.phase = Phase::Closed; }
    }
}

struct Accepted(ffi::IceConn);

impl Drop for Accepted {
    fn drop(&mut self) {
        // An admission future can be dropped while awaiting D-Bus ownership.
        // The connection remains ours until it has been transferred to Peer.
        if !self.0.is_null() { unsafe { ffi::IceCloseConnection(self.0); } }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        // SAFETY: callbacks clear sms after SmsCleanUp; process clears ice when
        // libICE has already freed it. Remaining references belong to this peer.
        unsafe {
            if !self.sms.is_null() { ffi::SmsCleanUp(self.sms); }
            if !self.ice.is_null() { ffi::IceCloseConnection(self.ice); }
        }
    }
}

fn credentials(fd: i32) -> Result<libc::ucred, SessionError> {
    let mut credentials = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut length = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: credentials and length cover the SO_PEERCRED result. Socket I/O
    // deadlines keep a partial protocol message from blocking the server.
    let result = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED,
        (&mut credentials as *mut libc::ucred).cast(), &mut length) };
    if result != 0 || length as usize != size_of::<libc::ucred>() {
        return Err(std::io::Error::last_os_error().into());
    }
    let timeout = libc::timeval { tv_sec: 0, tv_usec: 250_000 };
    for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
        if unsafe { libc::setsockopt(fd, libc::SOL_SOCKET, option, (&timeout as *const libc::timeval).cast(),
            size_of::<libc::timeval>() as libc::socklen_t) } != 0
        { return Err(std::io::Error::last_os_error().into()); }
    }
    Ok(credentials)
}

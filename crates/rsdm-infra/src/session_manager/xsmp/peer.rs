//! Socket credentials, pinned process identity and registered app ownership.

use std::{ptr, time::Instant};

use super::{ffi, properties::Properties, protocol::Phase};
use crate::session_manager::{SessionError, apps, bus::UserManager,
    processes::{ProcessHandle, app_processes}, runtime::Runtime};

pub(super) struct Peer {
    pub ice: ffi::IceConn,
    pub sms: ffi::SmsConn,
    pub unit: String,
    pub invocation: Vec<u8>,
    pub pid: u32,
    process: ProcessHandle,
    pub phase: Phase,
    pub selected: bool,
    pub interaction_requested: bool,
    pub interacting: bool,
    pub cancellation_requested: bool,
    pub local_save_requested: bool,
    pub deadline: u64,
    pub cancel_on_timeout: bool,
    pub properties: Properties,
    pub accepted: Instant,
}

impl Peer {
    pub fn accept(listener: ffi::ListenObj, manager: &UserManager, runtime: &Runtime) -> Result<Box<Self>, SessionError> {
        let mut status = 0;
        // SAFETY: the listener belongs to the server; a successful accept gives
        // this function one owned connection reference.
        let ice = unsafe { ffi::IceAcceptConnection(listener, &mut status) };
        if ice.is_null() { return Err(SessionError::State("ICE connection could not be accepted".into())); }
        unsafe { ffi::IceSetShutdownNegotiation(ice, 0); }
        let admission = Self::admit(ice, manager, runtime);
        if admission.is_err() {
            unsafe { ffi::IceCloseConnection(ice); }
        }
        admission
    }

    fn admit(ice: ffi::IceConn, manager: &UserManager, runtime: &Runtime) -> Result<Box<Self>, SessionError> {
        let credentials = credentials(unsafe { ffi::IceConnectionNumber(ice) })?;
        // SAFETY: geteuid has no preconditions.
        if credentials.uid != unsafe { libc::geteuid() } || credentials.pid <= 0 {
            return Err(SessionError::State("ICE peer belongs to another user".into()));
        }
        let pid = credentials.pid as u32;
        let process = ProcessHandle::open(pid)?.ok_or_else(|| SessionError::State("ICE peer process exited".into()))?;
        let unit = manager.unit_for_pid(pid)?;
        let mut app = runtime.app(&unit)?;
        if app.invocation_id.is_empty() {
            apps::pin_pending(manager, runtime, &unit)?;
            app = runtime.app(&unit)?;
        }
        if !process.alive()? || !app_processes(manager, &app)?.contains(&pid) {
            return Err(SessionError::State("ICE peer is not a registered application process".into()));
        }
        Ok(Box::new(Self {
            ice, sms: ptr::null_mut(), unit, invocation: app.invocation_id, pid, process,
            phase: Phase::Registering, selected: false, interaction_requested: false, interacting: false,
            cancellation_requested: false, local_save_requested: false, deadline: 0,
            cancel_on_timeout: false, properties: Properties::default(), accepted: Instant::now(),
        }))
    }

    pub fn verified(&self, manager: &UserManager, runtime: &Runtime) -> Result<bool, SessionError> {
        if self.phase == Phase::Closed || self.sms.is_null() || !self.process.alive()? { return Ok(false); }
        let app = runtime.app(&self.unit)?;
        Ok(app.invocation_id == self.invocation && app_processes(manager, &app)?.contains(&self.pid))
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

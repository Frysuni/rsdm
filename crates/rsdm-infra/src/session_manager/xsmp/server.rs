//! One thread owns all ICE objects, dispatch and SM library globals.

use std::{cell::Cell, collections::HashSet, ptr, sync::{atomic::{AtomicBool, Ordering}, mpsc::{Receiver, Sender}}, time::Duration};

use super::{Message, Notice, authority::Authority, callbacks::{self, Admission}, ffi,
    listeners::Listeners, peer::Peer, protocol::Phase};
use crate::session_manager::{SessionError, bus::UserManager, runtime::Runtime};

static INITIALIZED: AtomicBool = AtomicBool::new(false);

pub(super) struct Server {
    pub manager: UserManager,
    pub runtime: Runtime,
    pub peers: Vec<Box<Peer>>,
    listeners: Listeners,
    _authority: Authority,
    admission: Box<Admission>,
    messages: Receiver<Message>,
    pub notices: Sender<Notice>,
    pub cancellable: bool,
    announced: HashSet<String>,
}

pub(super) fn run(
    manager: UserManager, runtime: Runtime, messages: Receiver<Message>, notices: Sender<Notice>,
    ready: Sender<Result<Vec<(String, String)>, SessionError>>,
) {
    let mut server = match Server::create(manager, runtime, messages, notices) {
        Ok(server) => server,
        Err(error) => { let _ = ready.send(Err(error)); return; }
    };
    let environment = vec![
        ("SESSION_MANAGER".into(), server.listeners.networks.join(",")),
        ("ICEAUTHORITY".into(), server._authority.path.to_string_lossy().into_owned()),
    ];
    if ready.send(Ok(environment)).is_err() { return; }
    if let Err(error) = server.run() {
        let _ = server.notices.send(Notice::Failed(error.to_string()));
    }
}

impl Server {
    fn create(
        manager: UserManager, runtime: Runtime, messages: Receiver<Message>, notices: Sender<Notice>,
    ) -> Result<Self, SessionError> {
        // libSM/libICE have process-global callbacks and an append-only cookie
        // table. A coordinator process owns exactly one server lifetime.
        if INITIALIZED.swap(true, Ordering::SeqCst) {
            return Err(SessionError::State("an XSMP server was already initialized in this process".into()));
        }
        unsafe {
            ffi::IceSetIOErrorHandler(callbacks::io_error);
            ffi::IceSetErrorHandler(callbacks::protocol_error);
            ffi::SmsSetErrorHandler(callbacks::protocol_error);
        }
        let listeners = Listeners::open()?;
        if listeners.count() > 32 { return Err(SessionError::State("too many local ICE transports".into())); }
        let authority = Authority::create(&runtime.path, &listeners.networks)?;
        let admission = Box::new(Admission { peer: Cell::new(ptr::null_mut()) });
        let mut error: [libc::c_char; 256] = [0; 256];
        // SAFETY: the boxed callback context remains stable until every peer is
        // dropped. No host-based authentication fallback is registered.
        let initialized = unsafe { ffi::SmsInitialize(c"RSDM".as_ptr(), c"1".as_ptr(), callbacks::new_client,
            (&*admission as *const Admission).cast_mut().cast(), None, 256, error.as_mut_ptr()) };
        if initialized == 0 { return Err(SessionError::State("could not initialize XSMP".into())); }
        Ok(Self { manager, runtime, peers: Vec::new(), listeners, _authority: authority, admission,
            messages, notices, cancellable: true, announced: HashSet::new() })
    }

    fn run(&mut self) -> Result<(), SessionError> {
        loop {
            while let Ok(message) = self.messages.try_recv() {
                match message {
                    Message::Prepare { units, cancellable, reply } => {
                        let result = self.prepare(&units, cancellable).map_err(|error| error.to_string());
                        let _ = reply.try_send(result);
                    }
                    Message::Cancel => self.cancel(),
                    Message::Stop => return Ok(()),
                }
            }
            self.advance()?;
            self.poll()?;
            self.remove_closed();
            self.announce();
        }
    }

    fn poll(&mut self) -> Result<(), SessionError> {
        let mut descriptors: Vec<_> = (0..self.listeners.count()).map(|index| descriptor(self.listeners.fd(index)))
            .chain(self.peers.iter().map(|peer| descriptor(peer.fd()))).collect();
        // SAFETY: the vector contains all live listener/connection descriptors.
        let ready = unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as libc::nfds_t, 25) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::Interrupted { Ok(()) } else { Err(error.into()) };
        }
        let listeners = self.listeners.count();
        let peers = self.peers.len();
        for index in 0..peers {
            if descriptors[listeners + index].revents != 0 {
                let peer = (&mut *self.peers[index]) as *mut Peer;
                self.admission.peer.set(peer);
                unsafe { Peer::process(peer); }
                self.admission.peer.set(ptr::null_mut());
            }
        }
        for (index, descriptor) in descriptors.iter().take(listeners).enumerate() {
            if descriptor.revents & libc::POLLIN != 0 {
                match Peer::accept(self.listeners.object(index), &self.manager, &self.runtime) {
                    Ok(peer) if self.peers.len() < 256 => self.peers.push(peer),
                    Ok(_) => {},
                    Err(error) => tracing::debug!(%error, "rejected ICE peer"),
                }
            }
        }
        Ok(())
    }

    fn remove_closed(&mut self) {
        self.peers.retain(|peer| peer.phase != Phase::Closed
            && !(peer.phase == Phase::Registering && peer.accepted.elapsed() > Duration::from_secs(5)));
    }

    fn announce(&mut self) {
        let current: HashSet<_> = self.peers.iter().filter(|peer| !peer.sms.is_null() && peer.phase != Phase::Registering)
            .map(|peer| peer.unit.clone()).collect();
        for unit in current.difference(&self.announced) { let _ = self.notices.send(Notice::Connected(unit.clone())); }
        for unit in self.announced.difference(&current) { let _ = self.notices.send(Notice::Disconnected(unit.clone())); }
        self.announced = current;
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Callback contexts and cookies stay alive until every connection is
        // closed; libICE references no application objects after this point.
        self.peers.clear();
    }
}

fn descriptor(fd: i32) -> libc::pollfd {
    libc::pollfd { fd, events: libc::POLLIN, revents: 0 }
}

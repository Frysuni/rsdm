//! One thread owns all ICE objects, dispatch and SM library globals.

use std::{cell::Cell, collections::HashSet, os::fd::OwnedFd, ptr, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};

use tokio::sync::{mpsc::{UnboundedReceiver, UnboundedSender}, oneshot};

use super::{Message, Notice, authority::Authority, callbacks::{self, Admission}, ffi,
    interrupt::Interrupt, listeners::Listeners, peer::Peer, protocol::Phase, readiness};
use crate::{SessionError, bus::UserManager, runtime::Runtime};

static INITIALIZED: AtomicBool = AtomicBool::new(false);

pub(super) struct Server {
    pub manager: UserManager,
    pub runtime: Runtime,
    pub peers: Vec<Box<Peer>>,
    listeners: Listeners,
    _authority: Authority,
    admission: Box<Admission>,
    messages: UnboundedReceiver<Message>,
    pub notices: UnboundedSender<Notice>,
    pub cancellable: bool,
    announced: HashSet<String>,
    interrupt: Arc<Interrupt>,
    _listener_sockets: Vec<Arc<OwnedFd>>,
}

enum Event {
    Message(Option<Message>),
    Ready(usize),
    Tick,
}

pub(super) async fn run(
    manager: UserManager, runtime: Runtime, messages: UnboundedReceiver<Message>, notices: UnboundedSender<Notice>,
    ready: oneshot::Sender<Result<Vec<(String, String)>, SessionError>>, interrupt: Arc<Interrupt>,
) {
    let result = tokio::select! {
        biased;
        _ = interrupt.stopped() => Ok(()),
        result = async {
            let mut server = match Server::create(manager, runtime, messages, notices.clone(), interrupt.clone()).await {
                Ok(server) => server,
                Err(error) => { let _ = ready.send(Err(error)); return Ok(()); }
            };
            let environment = vec![
                ("SESSION_MANAGER".into(), server.listeners.networks.join(",")),
                ("ICEAUTHORITY".into(), server._authority.path.to_string_lossy().into_owned()),
            ];
            if ready.send(Ok(environment)).is_err() { return Ok(()); }
            server.run().await
        } => result,
    };
    if let Err(error) = result {
        let _ = notices.send(Notice::Failed(error.to_string()));
    }
}

impl Server {
    async fn create(
        manager: UserManager, runtime: Runtime, messages: UnboundedReceiver<Message>, notices: UnboundedSender<Notice>,
        interrupt: Arc<Interrupt>,
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
        let listener_sockets = (0..listeners.count()).map(|index| interrupt.register(listeners.fd(index)))
            .collect::<Result<Vec<_>, _>>()?;
        let authority = Authority::create(&runtime.path, &listeners.networks).await?;
        let admission = Box::new(Admission { peer: Cell::new(ptr::null_mut()) });
        let mut error: [libc::c_char; 256] = [0; 256];
        // SAFETY: the boxed callback context remains stable until every peer is
        // dropped. No host-based authentication fallback is registered.
        let initialized = unsafe { ffi::SmsInitialize(c"RSDM".as_ptr(), c"1".as_ptr(), callbacks::new_client,
            (&*admission as *const Admission).cast_mut().cast(), None, 256, error.as_mut_ptr()) };
        if initialized == 0 { return Err(SessionError::State("could not initialize XSMP".into())); }
        Ok(Self { manager, runtime, peers: Vec::new(), listeners, _authority: authority, admission,
            messages, notices, cancellable: true, announced: HashSet::new(), interrupt,
            _listener_sockets: listener_sockets })
    }

    async fn run(&mut self) -> Result<(), SessionError> {
        let mut tick = tokio::time::interval(Duration::from_millis(25));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            self.advance().await?;
            let descriptors: Vec<_> = (0..self.listeners.count()).map(|index| self.listeners.fd(index))
                .chain(self.peers.iter().map(|peer| peer.fd())).collect();
            let event = tokio::select! {
                message = self.messages.recv() => Event::Message(message),
                ready = readiness::wait(&descriptors) => Event::Ready(ready?),
                _ = tick.tick() => Event::Tick,
            };
            // All readiness registrations and their nonblocking flags are gone
            // before a branch can call libICE, including prepare/cancel writes.
            match event {
                Event::Message(Some(message)) => self.message(message).await?,
                Event::Message(None) => return Ok(()),
                Event::Ready(index) => self.process_ready(index).await?,
                Event::Tick => {},
            }
            self.remove_closed();
            self.announce();
        }
    }

    async fn message(&mut self, message: Message) -> Result<(), SessionError> {
        match message {
            Message::Prepare { units, cancellable, reply } => {
                let result = self.prepare(&units, cancellable).await.map_err(|error| error.to_string());
                let _ = reply.try_send(result);
            }
            Message::Cancel => self.cancel(),
        }
        Ok(())
    }

    async fn process_ready(&mut self, index: usize) -> Result<(), SessionError> {
        if index >= self.listeners.count() {
            let peer = (&mut *self.peers[index - self.listeners.count()]) as *mut Peer;
            self.admission.peer.set(peer);
            unsafe { Peer::process(peer); }
            self.admission.peer.set(ptr::null_mut());
            return Ok(());
        }
        match Peer::accept(self.listeners.object(index), &self.manager, &self.runtime, &self.interrupt).await {
            Ok(peer) if self.peers.len() < 256 => self.peers.push(peer),
            Ok(_) => {},
            Err(error) => tracing::debug!(%error, "rejected ICE peer"),
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

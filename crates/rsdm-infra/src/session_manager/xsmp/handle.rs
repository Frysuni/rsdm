//! Thread ownership and requests for the optional XSMP backend.

use std::{collections::HashSet, sync::mpsc::{self, Receiver, Sender}, thread::JoinHandle};

use super::server;
use crate::session_manager::{SessionError, bus::UserManager, control::Reply,
    provider::{Provider, ProviderKind}, runtime::Runtime};

pub enum Notice {
    Connected(String),
    Disconnected(String),
    Cancelled,
    Failed(String),
}

pub enum Message {
    Prepare { units: Vec<String>, cancellable: bool, reply: Reply<Vec<String>> },
    Cancel,
    Stop,
}

pub struct Handle {
    sender: Option<Sender<Message>>,
    thread: Option<JoinHandle<()>>,
    notices: Receiver<Notice>,
    pub connected: HashSet<String>,
    pub environment: Vec<(String, String)>,
}

impl Handle {
    pub fn start(manager: &UserManager, runtime: &Runtime, provider: &Provider) -> Result<Self, SessionError> {
        let (sender, messages) = mpsc::channel();
        let (events, notices) = mpsc::channel();
        let mut handle = Self { sender: None, thread: None, notices, connected: HashSet::new(), environment: Vec::new() };
        if !matches!(provider.kind, ProviderKind::Managed | ProviderKind::Niri) { return Ok(handle); }

        let (ready, started) = mpsc::channel();
        let manager = manager.clone();
        let runtime = runtime.clone();
        let worker = std::thread::spawn(move || server::run(manager, runtime, messages, events, ready));
        match started.recv() {
            Ok(Ok(environment)) => {
                handle.environment = environment;
                handle.sender = Some(sender);
                handle.thread = Some(worker);
            }
            Ok(Err(error)) => { let _ = worker.join(); return Err(error); }
            Err(_) => { let _ = worker.join(); return Err(SessionError::State("XSMP server failed during startup".into())); }
        }
        Ok(handle)
    }

    pub fn available(&self) -> bool { self.sender.is_some() }

    pub fn prepare(&self, units: Vec<String>, cancellable: bool, reply: Reply<Vec<String>>) {
        if let Some(sender) = &self.sender {
            if sender.send(Message::Prepare { units, cancellable, reply: reply.clone() }).is_err() {
                let _ = reply.try_send(Err("XSMP server stopped".into()));
            }
        } else { let _ = reply.try_send(Ok(Vec::new())); }
    }

    pub fn cancel(&self) {
        if let Some(sender) = &self.sender { let _ = sender.send(Message::Cancel); }
    }

    pub fn poll(&mut self) -> Option<Notice> {
        let notice = self.notices.try_recv().ok()?;
        match &notice {
            Notice::Connected(unit) => { self.connected.insert(unit.clone()); }
            Notice::Disconnected(unit) => { self.connected.remove(unit); }
            _ => {},
        }
        Some(notice)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Some(sender) = &self.sender { let _ = sender.send(Message::Stop); }
        if let Some(worker) = self.thread.take() { let _ = worker.join(); }
    }
}

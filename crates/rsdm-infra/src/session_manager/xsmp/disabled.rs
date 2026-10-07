//! Custom builds without libSM/libICE retain the session lifecycle API.

use std::collections::HashSet;

use crate::session_manager::{SessionError, bus::UserManager, control::Reply,
    provider::Provider, runtime::Runtime};

pub struct Handle {
    pub connected: HashSet<String>,
    pub environment: Vec<(String, String)>,
}

impl Handle {
    pub fn start(_manager: &UserManager, _runtime: &Runtime, _provider: &Provider) -> Result<Self, SessionError> {
        Ok(Self { connected: HashSet::new(), environment: Vec::new() })
    }

    pub fn available(&self) -> bool { false }

    pub fn prepare(&self, _units: Vec<String>, _cancellable: bool, reply: Reply<Vec<String>>) {
        let _ = reply.try_send(Ok(Vec::new()));
    }

    pub fn cancel(&self) {}
}

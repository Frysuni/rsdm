//! A disabled generation cannot dispatch further external actions.

use std::{future::Future, sync::{Arc, atomic::{AtomicBool, Ordering}}};

use rsdm_core::domain::SessionManagerUpdate;
use tokio::sync::{Notify, watch};

use super::SessionError;

#[derive(Clone, Default)]
pub(super) struct ActionGate {
    closed: Arc<AtomicBool>,
    changed: Arc<Notify>,
    config: Option<watch::Receiver<SessionManagerUpdate>>,
    disable_count: u64,
}

#[cfg(test)]
#[path = "action_gate_tests.rs"]
mod tests;

impl ActionGate {
    pub fn new(config: Option<watch::Receiver<SessionManagerUpdate>>) -> Self {
        let disable_count = config.as_ref().map_or(0, |receiver| receiver.borrow().disable_count);
        Self { config, disable_count, ..Self::default() }
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }

    pub fn check(&self) -> Result<(), SessionError> {
        if self.config.as_ref().is_some_and(|receiver| {
            let update = receiver.borrow();
            !update.config.enabled || update.disable_count != self.disable_count
        }) { self.close(); }
        if self.closed.load(Ordering::Acquire) { return Err(SessionError::Disabled); }
        Ok(())
    }

    pub async fn wait_closed(&self) {
        let mut config = self.config.clone();
        loop {
            let notified = self.changed.notified();
            if self.check().is_err() { return; }
            tokio::select! {
                _ = notified => {}
                result = async { config.as_mut().expect("configuration receiver").changed().await }, if config.is_some() => {
                    // The last validated value remains authoritative if the
                    // loader is gone; explicit closure still wakes every clone.
                    if result.is_err() { config = None; }
                }
            }
        }
    }

    pub async fn guard<T>(&self, operation: impl Future<Output = T>) -> Result<T, SessionError> {
        tokio::select! {
            biased;
            _ = self.wait_closed() => Err(SessionError::Disabled),
            result = operation => Ok(result),
        }
    }
}

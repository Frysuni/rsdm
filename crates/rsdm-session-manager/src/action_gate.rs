//! A disabled generation cannot dispatch further external actions.

use std::{future::Future, sync::{Arc, atomic::{AtomicBool, Ordering}}};

use rsdm_core::domain::SessionManagerUpdate;
use tokio::sync::{Notify, watch};

use super::{SessionError, runtime::{blocking, Runtime}};

#[derive(Clone, Default)]
pub(super) struct ActionGate {
    closed: Arc<AtomicBool>,
    changed: Arc<Notify>,
    config: Option<watch::Receiver<SessionManagerUpdate>>,
    disable_count: u64,
    generation: Option<Runtime>,
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

    pub fn with_generation(mut self, runtime: Runtime) -> Self {
        self.generation = Some(runtime);
        self
    }

    pub fn check_generation(&self) -> Result<(), SessionError> {
        self.check()?;
        if let Some(runtime) = &self.generation {
            if runtime.is_disabled()? { self.close(); }
        }
        self.check()
    }

    pub async fn check_generation_async(&self) -> Result<(), SessionError> {
        self.check()?;
        if self.generation.is_none() { return Ok(()); }
        let actions = self.clone();
        blocking(move || actions.check_generation()).await
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
        let mut retirement = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            let notified = self.changed.notified();
            if self.check().is_err() { return; }
            tokio::select! {
                _ = notified => {}
                result = async {
                    retirement.tick().await;
                    self.check_generation_async().await
                }, if self.generation.is_some() => {
                    // Dispatch checks propagate storage errors. This observer
                    // only interrupts pending work after confirmed retirement.
                    let _ = result;
                }
                result = async { config.as_mut().expect("configuration receiver").changed().await }, if config.is_some() => {
                    // The last validated value remains authoritative if the
                    // loader is gone; explicit closure still wakes every clone.
                    if result.is_err() { config = None; }
                }
            }
        }
    }

    pub fn guard<T>(&self, operation: impl Future<Output = T>) -> impl Future<Output = Result<T, SessionError>> {
        // Recovery composes several guards; allocate before returning the
        // future so each caller retains only a pointer to the guarded work.
        let operation = Box::pin(async move {
            self.check_generation_async().await?;
            Ok(operation.await)
        });
        async move {
            tokio::select! {
                biased;
                _ = self.wait_closed() => Err(SessionError::Disabled),
                result = operation => result,
            }
        }
    }
}

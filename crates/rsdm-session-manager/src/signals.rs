//! Termination signals are consumed by the serial lifecycle actor.

use tokio::signal::unix::{Signal, SignalKind, signal};

use super::SessionError;

pub(super) struct Signals {
    terminate: Signal,
    interrupt: Signal,
}

impl Signals {
    pub(super) fn new() -> Result<Self, SessionError> {
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    pub(super) async fn recv(&mut self) -> Result<(), SessionError> {
        let received = tokio::select! {
            received = self.terminate.recv() => received,
            received = self.interrupt.recv() => received,
        };
        received.ok_or_else(|| SessionError::State("session termination signal stream closed".into()))
    }
}

#[cfg(test)]
#[path = "signals_tests.rs"]
mod tests;

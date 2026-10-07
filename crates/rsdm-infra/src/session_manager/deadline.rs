//! A shared monotonic deadline that wakes all pending operations when revised.

use std::{future::Future, sync::{Arc, Mutex}, time::Duration};

use async_channel::{Receiver, Sender};
use futures_lite::future;

#[derive(Clone, Default)]
pub(super) struct Deadline(Arc<Mutex<State>>);

struct State {
    usec: u64,
    changed: (Sender<()>, Receiver<()>),
}

impl Default for State {
    fn default() -> Self { Self { usec: 0, changed: async_channel::bounded(1) } }
}

impl Deadline {
    pub fn get(&self) -> u64 {
        self.0.lock().unwrap_or_else(|error| error.into_inner()).usec
    }

    pub fn set(&self, usec: u64) {
        let old = {
            let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
            if state.usec == usec { return; }
            state.usec = usec;
            std::mem::replace(&mut state.changed, async_channel::bounded(1))
        };
        // Closing wakes every receiver clone, rather than notifying just one
        // operation. Each waiter then snapshots the new deadline and channel.
        old.0.close();
    }

    pub fn remaining(&self, maximum: Duration) -> zbus::Result<Duration> {
        remaining_until(self.get(), maximum)
    }

    pub async fn bound<T, E>(
        &self, operation: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E>
    where E: From<zbus::Error> {
        self.remaining(Duration::MAX)?;
        future::or(operation, async { Err(self.expired().await.into()) }).await
    }

    async fn expired(&self) -> zbus::Error {
        loop {
            let (usec, changed) = {
                let state = self.0.lock().unwrap_or_else(|error| error.into_inner());
                (state.usec, state.changed.1.clone())
            };
            if usec == 0 {
                let _ = changed.recv().await;
                continue;
            }
            let remaining = match remaining_until(usec, Duration::MAX) {
                Ok(remaining) => remaining,
                Err(error) if self.get() == usec => return error,
                Err(_) => continue,
            };
            future::or(async { async_io::Timer::after(remaining).await; },
                async { let _ = changed.recv().await; }).await;
        }
    }
}

fn remaining_until(usec: u64, maximum: Duration) -> zbus::Result<Duration> {
    if usec == 0 { return Ok(maximum); }
    let now = super::processes::monotonic_usec().map_err(|error| zbus::Error::Failure(error.to_string()))?;
    let remaining = Duration::from_micros(usec.saturating_sub(now)).min(maximum);
    if remaining.is_zero() {
        return Err(zbus::fdo::Error::TimedOut("session shutdown deadline expired".into()).into());
    }
    Ok(remaining)
}

#[cfg(test)]
#[path = "deadline_tests.rs"]
mod tests;

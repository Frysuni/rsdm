//! A shared monotonic deadline that wakes all pending operations when revised.

use std::{future::Future, time::Duration};

use tokio::sync::watch;

#[derive(Clone)]
pub(super) struct Deadline {
    state: watch::Sender<u64>,
    reserve_usec: u64,
}

impl Default for Deadline {
    fn default() -> Self { Self { state: watch::channel(0).0, reserve_usec: 0 } }
}

impl Deadline {
    pub fn get(&self) -> u64 {
        self.effective(*self.state.borrow())
    }

    // A phase shares revisions and revocation with the absolute deadline, but
    // stops earlier so later teardown still has time to make progress.
    pub fn reserving(&self, maximum: Duration) -> zbus::Result<Self> {
        let reserve = if self.get() == 0 { maximum } else {
            maximum.min(self.remaining(Duration::MAX)? / 2)
        };
        let reserve_usec = u64::try_from(reserve.as_micros()).unwrap_or(u64::MAX);
        Ok(Self { state: self.state.clone(), reserve_usec: self.reserve_usec.saturating_add(reserve_usec) })
    }

    fn effective(&self, usec: u64) -> u64 {
        if usec == 0 { 0 } else { usec.saturating_sub(self.reserve_usec).max(1) }
    }

    pub fn set(&self, usec: u64) {
        self.state.send_if_modified(|current| {
            if *current == usec { return false; }
            *current = usec;
            true
        });
    }

    pub fn remaining(&self, maximum: Duration) -> zbus::Result<Duration> {
        remaining_until(self.get(), maximum)
    }

    pub async fn bound<T, E>(
        &self, operation: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E>
    where E: From<zbus::Error> {
        self.remaining(Duration::MAX)?;
        tokio::select! {
            biased;
            result = operation => result,
            error = self.expired() => Err(error.into()),
        }
    }

    async fn expired(&self) -> zbus::Error {
        let mut changed = self.state.subscribe();
        loop {
            let usec = self.effective(*changed.borrow_and_update());
            if usec == 0 {
                let _ = changed.changed().await;
                continue;
            }
            let remaining = match remaining_until(usec, Duration::MAX) {
                Ok(remaining) => remaining,
                Err(error) if self.get() == usec => return error,
                Err(_) => continue,
            };
            tokio::select! {
                _ = tokio::time::sleep(remaining) => {},
                _ = changed.changed() => {},
            }
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

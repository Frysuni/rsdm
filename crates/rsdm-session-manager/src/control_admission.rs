//! Admission follows a request through queueing, workers, and reply delivery.

use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};

use tokio::sync::mpsc::{Sender, error::TrySendError};

pub(crate) const MAX_REGULAR_REQUESTS: usize = 64;
const MAX_STOP_REQUESTS: usize = 4;
const MAX_CANCEL_REQUESTS: usize = 2;
const MAX_XSMP_REQUESTS: usize = 2;
pub(super) const QUEUE_CAPACITY: usize = MAX_REGULAR_REQUESTS
    + MAX_STOP_REQUESTS + MAX_CANCEL_REQUESTS + MAX_XSMP_REQUESTS;

pub(super) enum Class {
    Regular,
    Stop,
    Cancel,
    Xsmp,
}

#[derive(Clone, Default)]
pub(super) struct Admission {
    closed: Arc<AtomicBool>,
    regular: Arc<AtomicUsize>,
    stop: Arc<AtomicUsize>,
    cancel: Arc<AtomicUsize>,
    xsmp: Arc<AtomicUsize>,
}

impl Admission {
    pub fn close(&self) { self.closed.store(true, Ordering::Release); }

    pub fn check_open(&self) -> zbus::fdo::Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(zbus::fdo::Error::Failed("session manager is disabled".into()));
        }
        Ok(())
    }

    pub fn acquire(&self, class: Class) -> zbus::fdo::Result<Arc<Permit>> {
        self.check_open()?;
        let (pending, limit) = match class {
            Class::Regular => (&self.regular, MAX_REGULAR_REQUESTS),
            Class::Stop => (&self.stop, MAX_STOP_REQUESTS),
            Class::Cancel => (&self.cancel, MAX_CANCEL_REQUESTS),
            Class::Xsmp => (&self.xsmp, MAX_XSMP_REQUESTS),
        };
        pending.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            if count >= limit { return None; }
            Some(count + 1)
        }).map_err(|_| zbus::fdo::Error::LimitsExceeded("session coordinator is busy; retry the request".into()))?;
        let permit = Arc::new(Permit { pending: pending.clone() });
        self.check_open()?;
        Ok(permit)
    }
}

pub(crate) struct Permit {
    pending: Arc<AtomicUsize>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.pending.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) struct Reply<T> {
    sender: Sender<Result<T, String>>,
    _permit: Arc<Permit>,
}

impl<T> Reply<T> {
    pub(super) fn new(sender: Sender<Result<T, String>>, permit: Arc<Permit>) -> Self {
        Self { sender, _permit: permit }
    }

    pub fn try_send(&self, result: Result<T, String>) -> Result<(), TrySendError<Result<T, String>>> {
        self.sender.try_send(result)
    }

    #[cfg(test)]
    pub fn for_test(sender: Sender<Result<T, String>>) -> Self {
        let permit = Admission::default().acquire(Class::Regular).unwrap();
        Self::new(sender, permit)
    }
}

impl<T> Clone for Reply<T> {
    fn clone(&self) -> Self {
        Self { sender: self.sender.clone(), _permit: self._permit.clone() }
    }
}

#[cfg(test)]
#[path = "control_admission_tests.rs"]
mod tests;

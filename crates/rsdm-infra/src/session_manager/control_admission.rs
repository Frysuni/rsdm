//! Admission follows a request through queueing, workers, and reply delivery.

use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

pub(in crate::session_manager) const MAX_REGULAR_REQUESTS: usize = 64;
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
    regular: Arc<AtomicUsize>,
    stop: Arc<AtomicUsize>,
    cancel: Arc<AtomicUsize>,
    xsmp: Arc<AtomicUsize>,
}

impl Admission {
    pub fn acquire(&self, class: Class) -> zbus::fdo::Result<Arc<Permit>> {
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
        Ok(Arc::new(Permit { pending: pending.clone() }))
    }
}

pub(in crate::session_manager) struct Permit {
    pending: Arc<AtomicUsize>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.pending.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(in crate::session_manager) struct Reply<T> {
    sender: async_channel::Sender<Result<T, String>>,
    _permit: Arc<Permit>,
}

impl<T> Reply<T> {
    pub(super) fn new(sender: async_channel::Sender<Result<T, String>>, permit: Arc<Permit>) -> Self {
        Self { sender, _permit: permit }
    }

    pub fn try_send(&self, result: Result<T, String>) -> Result<(), async_channel::TrySendError<Result<T, String>>> {
        self.sender.try_send(result)
    }

    #[cfg(test)]
    pub fn send_blocking(&self, result: Result<T, String>) -> Result<(), async_channel::SendError<Result<T, String>>> {
        self.sender.send_blocking(result)
    }

    #[cfg(test)]
    pub fn for_test(sender: async_channel::Sender<Result<T, String>>) -> Self {
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

//! Thread ownership and requests for the optional XSMP backend.

use std::{collections::HashSet, sync::Arc, time::Duration};

use tokio::sync::{mpsc::{self, UnboundedReceiver, UnboundedSender}, oneshot};

use super::{interrupt::Interrupt, server};
use crate::{SessionError, bus::UserManager, control::Reply, deadline::Deadline,
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
}

pub struct Handle {
    sender: Option<UnboundedSender<Message>>,
    finished: Option<oneshot::Receiver<()>>,
    notices: UnboundedReceiver<Notice>,
    interrupt: Arc<Interrupt>,
    pub connected: HashSet<String>,
    pub environment: Vec<(String, String)>,
}

impl Handle {
    pub async fn start(manager: &UserManager, runtime: &Runtime, provider: &Provider) -> Result<Self, SessionError> {
        let (sender, messages) = mpsc::unbounded_channel();
        let (events, notices) = mpsc::unbounded_channel();
        let mut handle = Self { sender: None, finished: None, notices, interrupt: Arc::default(),
            connected: HashSet::new(), environment: Vec::new() };
        if !matches!(provider.kind, ProviderKind::Managed | ProviderKind::Niri) { return Ok(handle); }
        manager.deadline.remaining(Duration::MAX)?;

        let (ready, started) = oneshot::channel();
        let (finished, completion) = oneshot::channel();
        handle.sender = Some(sender);
        handle.finished = Some(completion);
        let interrupt = handle.interrupt.clone();
        let worker_manager = manager.clone();
        let runtime = runtime.clone();
        // The completion channel replaces join: dropping a pending startup
        // still interrupts the worker, and no caller blocks on its thread.
        let _ = std::thread::spawn(move || {
            match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(executor) => executor.block_on(server::run(worker_manager, runtime, messages, events, ready, interrupt)),
                Err(error) => { let _ = ready.send(Err(error.into())); }
            }
            let _ = finished.send(());
        });
        handle.await_startup(started, &manager.deadline).await?;
        Ok(handle)
    }

    async fn await_startup(
        &mut self, started: oneshot::Receiver<Result<Vec<(String, String)>, SessionError>>, deadline: &Deadline,
    ) -> Result<(), SessionError> {
        self.environment = deadline.bound(async {
            tokio::time::timeout(Duration::from_secs(5), started).await
                .map_err(|_| SessionError::State("XSMP startup timed out".into()))?
                .map_err(|_| SessionError::State("XSMP server failed during startup".into()))?
        }).await?;
        Ok(())
    }

    pub async fn shutdown(&mut self, deadline: &Deadline) -> Result<(), SessionError> {
        self.sender.take();

        let Some(finished) = self.finished.as_mut() else {
            self.interrupt.stop();
            return Ok(());
        };
        if finished.try_recv().is_ok() {
            self.interrupt.stop();
            self.finished.take();
            return Ok(());
        }

        let limit = tokio::time::Instant::now() + Duration::from_millis(500);
        let interrupt = &self.interrupt;
        let result = deadline.bound(async {
            let mut completion = std::pin::pin!(async {
                finished.await.map_err(|_| SessionError::State("XSMP worker failed during shutdown".into()))
            });
            tokio::select! {
                result = &mut completion => result,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    // Responsive workers can first send ShutdownCancelled.
                    // Stalled FFI or admission still uses the same total budget.
                    interrupt.stop();
                    tokio::time::timeout_at(limit, completion).await
                        .map_err(|_| SessionError::State("XSMP worker shutdown timed out".into()))?
                }
            }
        }).await;
        self.interrupt.stop();
        if result.is_ok() { self.finished.take(); }
        result
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
        self.interrupt.stop();
    }
}

#[cfg(test)]
#[path = "handle_tests.rs"]
mod tests;

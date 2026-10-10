//! Logind shutdown notifications and an optional delay inhibitor.

use std::{os::fd::OwnedFd, sync::mpsc::Sender, thread, time::Duration};

use futures_lite::{StreamExt, future};
use zbus::Connection;

use super::{LogindError, connection, inhibit, proxy};

#[derive(Debug)]
pub struct ShutdownNotice {
    pub preparing: bool,
    pub deadline_usec: u64,
}

impl ShutdownNotice {
    pub fn new(preparing: bool, budget_usec: u64) -> Self {
        let deadline_usec = if preparing {
            crate::clock::monotonic_usec().map(|now| now.saturating_add(budget_usec))
                .unwrap_or_else(|error| {
                    tracing::error!(%error, "cannot timestamp shutdown notice; treating budget as expired");
                    1
                })
        } else { 0 };
        Self { preparing, deadline_usec }
    }
}

pub struct ShutdownMonitor {
    pub(super) _inhibitor: Option<OwnedFd>,
    stop: async_channel::Sender<()>,
    listener: Option<thread::JoinHandle<()>>,
}

impl ShutdownMonitor {
    pub fn start(notices: Sender<ShutdownNotice>, reason: &str) -> Result<Self, LogindError> {
        async_io::block_on(async {
            Self::listen(connection(Duration::from_secs(5)).await?, notices, reason).await
        })
    }

    pub(super) async fn listen(connection: Connection, notices: Sender<ShutdownNotice>, reason: &str) -> Result<Self, LogindError> {
        // Holding the FD precedes the signal subscription and the initial state query.
        let inhibitor = match inhibit(&connection, reason).await {
            Ok(fd) => Some(fd),
            Err(error) => {
                tracing::warn!(%error, "user shutdown delay inhibitor unavailable; continuing shutdown notifications without an inhibitor");
                None
            }
        };
        let proxy = proxy(&connection).await?;
        let mut signals = proxy.receive_signal("PrepareForShutdown").await?;
        let maximum: u64 = proxy.get_property("InhibitDelayMaxUSec").await?;
        let budget_usec = maximum.saturating_sub(250_000);
        let initial = ShutdownNotice::new(true, budget_usec);
        let preparing: bool = proxy.get_property("PreparingForShutdown").await?;
        if preparing { let _ = notices.send(initial); }
        let (stop, stopped) = async_channel::bounded(1);
        let listener = thread::spawn(move || async_io::block_on(async move {
            future::or(async {
                while let Some(signal) = signals.next().await {
                    match signal.body().deserialize::<bool>() {
                        Ok(preparing) => { if notices.send(ShutdownNotice::new(preparing, budget_usec)).is_err() { break; } }
                        Err(error) => tracing::warn!(%error, "invalid logind shutdown notification"),
                    }
                }
            }, async { let _ = stopped.recv().await; }).await;
            drop(connection);
        }));
        Ok(Self { _inhibitor: inhibitor, stop, listener: Some(listener) })
    }
}

impl Drop for ShutdownMonitor {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(listener) = self.listener.take() { let _ = listener.join(); }
    }
}

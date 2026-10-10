//! Logind shutdown notifications and an optional delay inhibitor.

use std::{future::poll_fn, os::fd::OwnedFd, pin::Pin, time::Duration};

use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};
use zbus::{Connection, export::futures_core::Stream};

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
    listener: JoinHandle<()>,
}

impl ShutdownMonitor {
    pub async fn start(notices: UnboundedSender<ShutdownNotice>, reason: &str) -> Result<Self, LogindError> {
        let timeout = Duration::from_secs(5);
        super::within_timeout(async {
            Self::listen(connection(timeout).await?, notices, reason).await
        }, timeout).await
    }

    pub(super) async fn listen(connection: Connection, notices: UnboundedSender<ShutdownNotice>, reason: &str) -> Result<Self, LogindError> {
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
        let listener = tokio::spawn(async move {
            while let Some(signal) = poll_fn(|cx| Pin::new(&mut signals).poll_next(cx)).await {
                match signal.body().deserialize::<bool>() {
                    Ok(preparing) => { if notices.send(ShutdownNotice::new(preparing, budget_usec)).is_err() { break; } }
                    Err(error) => tracing::warn!(%error, "invalid logind shutdown notification"),
                }
            }
            drop(connection);
        });
        Ok(Self { _inhibitor: inhibitor, listener })
    }
}

impl Drop for ShutdownMonitor {
    fn drop(&mut self) {
        self.listener.abort();
    }
}

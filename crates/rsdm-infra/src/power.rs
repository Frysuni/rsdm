//! Logind power authorization and delay inhibitors, without privilege escalation.

use std::{future::Future, os::fd::OwnedFd, sync::mpsc::Sender, thread, time::Duration};

use futures_lite::{StreamExt, future};
use zbus::{Connection, Proxy};

use crate::session_manager::{SessionError, StopOutcome};

const DESTINATION: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const INTERFACE: &str = "org.freedesktop.login1.Manager";

async fn connection(timeout: Duration) -> Result<Connection, SessionError> {
    Ok(zbus::connection::Builder::system()?.method_timeout(timeout).build().await?)
}

async fn proxy(connection: &Connection) -> Result<Proxy<'_>, zbus::Error> {
    Proxy::new(connection, DESTINATION, PATH, INTERFACE).await
}

fn method(action: &str) -> Result<&'static str, SessionError> {
    match action {
        "reboot" => Ok("Reboot"), "poweroff" => Ok("PowerOff"),
        "suspend" => Ok("Suspend"), "hibernate" => Ok("Hibernate"),
        _ => Err(SessionError::State("unsupported power action".into())),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerCapabilities {
    pub hibernate: bool,
    pub suspend: bool,
}

/// Query the same logind authority used for executing sleep requests.
pub fn capabilities() -> Result<PowerCapabilities, SessionError> {
    let timeout = Duration::from_secs(5);
    async_io::block_on(query_capabilities(connection(timeout), timeout))
}

async fn query_capabilities(
    connect: impl Future<Output = Result<Connection, SessionError>>, timeout: Duration,
) -> Result<PowerCapabilities, SessionError> {
    future::or(async {
        let connection = connect.await?;
        let proxy = proxy(&connection).await?;
        let (hibernate, suspend) = future::zip(
            available(&proxy, "CanHibernate"), available(&proxy, "CanSuspend"),
        ).await;
        Ok(PowerCapabilities { hibernate, suspend })
    }, async {
        async_io::Timer::after(timeout).await;
        Err(SessionError::State("logind power capability query timed out".into()))
    }).await
}

async fn available(proxy: &Proxy<'_>, method: &str) -> bool {
    match proxy.call::<_, _, String>(method, &()).await {
        Ok(result) => matches!(result.as_str(), "yes" | "challenge"),
        Err(error) => {
            tracing::warn!(%method, %error, "logind power capability unavailable");
            false
        }
    }
}

pub fn check(action: &str) -> Result<(), SessionError> {
    async_io::block_on(check_async(action))
}

pub(crate) async fn check_async(action: &str) -> Result<(), SessionError> {
    let timeout = Duration::from_secs(5);
    check_with(connection(timeout), action, timeout).await
}

async fn check_with(
    connect: impl Future<Output = Result<Connection, SessionError>>, action: &str, timeout: Duration,
) -> Result<(), SessionError> {
    let method = format!("Can{}", method(action)?);
    within_timeout(async {
        let connection = connect.await?;
        let result: String = proxy(&connection).await?.call(method.as_str(), &()).await?;
        if matches!(result.as_str(), "yes" | "challenge") { return Ok(()); }
        Err(SessionError::State(format!("logind does not allow {action}: {result}")))
    }, timeout).await
}

pub fn request_direct(action: &str) -> Result<(), SessionError> {
    async_io::block_on(request_direct_async(action))
}

pub(crate) async fn request_direct_async(action: &str) -> Result<(), SessionError> {
    let timeout = Duration::from_secs(60);
    request_with(connection(timeout), action, timeout).await
}

async fn request_with(
    connect: impl Future<Output = Result<Connection, SessionError>>, action: &str, timeout: Duration,
) -> Result<(), SessionError> {
    let method = method(action)?;
    within_timeout(async {
        let connection = connect.await?;
        proxy(&connection).await?.call::<_, _, ()>(method, &(true,)).await?;
        Ok(())
    }, timeout).await
}

async fn within_timeout<T>(
    operation: impl Future<Output = Result<T, SessionError>>, timeout: Duration,
) -> Result<T, SessionError> {
    future::or(operation, async {
        async_io::Timer::after(timeout).await;
        Err(zbus::Error::from(zbus::fdo::Error::TimedOut("logind request timed out".into())).into())
    }).await
}

pub fn request(action: &str) -> Result<StopOutcome, SessionError> {
    method(action)?;
    if matches!(action, "reboot" | "poweroff") && std::env::var_os("RSDM_SESSION_GENERATION").is_some() {
        return crate::session_manager::stop(action);
    }
    request_direct(action)?;
    Ok(StopOutcome { result: "accepted".into(), forced_units: Vec::new(), message: format!("logind accepted {action}") })
}

async fn inhibit(connection: &Connection, reason: &str) -> Result<OwnedFd, SessionError> {
    let fd: zbus::zvariant::OwnedFd = proxy(connection).await?
        .call("Inhibit", &("shutdown", "RSDM", reason, "delay")).await?;
    Ok(fd.into())
}

/// The PAM session owner keeps this descriptor until pam_close_session finishes.
/// Call only after the session child has forked.
pub fn pam_shutdown_guard() -> Result<OwnedFd, SessionError> {
    async_io::block_on(async {
        let connection = connection(Duration::from_secs(5)).await?;
        inhibit(&connection, "Closing the graphical PAM session").await
    })
}

#[derive(Debug)]
pub(crate) struct ShutdownNotice {
    pub preparing: bool,
    pub deadline_usec: u64,
}

impl ShutdownNotice {
    pub(crate) fn new(preparing: bool, budget_usec: u64) -> Self {
        let deadline_usec = if preparing {
            crate::session_manager::monotonic_usec().map(|now| now.saturating_add(budget_usec))
                .unwrap_or_else(|error| {
                    tracing::error!(%error, "cannot timestamp shutdown notice; treating budget as expired");
                    1
                })
        } else { 0 };
        Self { preparing, deadline_usec }
    }
}

pub(crate) struct ShutdownMonitor {
    _inhibitor: Option<OwnedFd>,
    stop: async_channel::Sender<()>,
    listener: Option<thread::JoinHandle<()>>,
}

impl ShutdownMonitor {
    #[cfg(test)]
    pub(crate) fn idle_for_test() -> Self {
        let (stop, _) = async_channel::bounded(1);
        Self { _inhibitor: None, stop, listener: None }
    }

    pub(crate) fn start(notices: Sender<ShutdownNotice>) -> Result<Self, SessionError> {
        async_io::block_on(async {
            Self::listen(connection(Duration::from_secs(5)).await?, notices).await
        })
    }

    async fn listen(connection: Connection, notices: Sender<ShutdownNotice>) -> Result<Self, SessionError> {
        // Holding the FD precedes the signal subscription and the initial state query.
        let inhibitor = match inhibit(&connection, "Saving registered graphical applications").await {
            Ok(fd) => Some(fd),
            Err(error) => {
                tracing::warn!(%error, "user shutdown delay inhibitor unavailable; external shutdown depends on the PAM owner's guard");
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

#[cfg(test)]
#[path = "power_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "power_capability_tests.rs"]
mod capability_tests;

#[cfg(test)]
#[path = "power_request_tests.rs"]
mod request_tests;

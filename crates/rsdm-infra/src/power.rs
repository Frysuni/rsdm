//! Logind power authorization and delay inhibitors, without privilege escalation.

use std::{os::fd::OwnedFd, sync::mpsc::Sender, thread, time::Duration};

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

pub fn check(action: &str) -> Result<(), SessionError> {
    let method = format!("Can{}", method(action)?);
    async_io::block_on(async {
        let connection = connection(Duration::from_secs(5)).await?;
        let result: String = proxy(&connection).await?.call(method.as_str(), &()).await?;
        if matches!(result.as_str(), "yes" | "challenge") { return Ok(()); }
        Err(SessionError::State(format!("logind does not allow {action}: {result}")))
    })
}

pub fn request_direct(action: &str) -> Result<(), SessionError> {
    let method = method(action)?;
    async_io::block_on(async {
        let connection = connection(Duration::from_secs(60)).await?;
        proxy(&connection).await?.call::<_, _, ()>(method, &(true,)).await?;
        Ok(())
    })
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
    pub budget_usec: u64,
}

pub(crate) struct ShutdownMonitor {
    _inhibitor: Option<OwnedFd>,
    stop: async_channel::Sender<()>,
    listener: Option<thread::JoinHandle<()>>,
}

impl ShutdownMonitor {
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
        let preparing: bool = proxy.get_property("PreparingForShutdown").await?;
        let budget_usec = maximum.saturating_sub(250_000);
        if preparing { let _ = notices.send(ShutdownNotice { preparing, budget_usec }); }
        let (stop, stopped) = async_channel::bounded(1);
        let listener = thread::spawn(move || async_io::block_on(async move {
            future::or(async {
                while let Some(signal) = signals.next().await {
                    match signal.body().deserialize::<bool>() {
                        Ok(preparing) => { if notices.send(ShutdownNotice { preparing, budget_usec }).is_err() { break; } }
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

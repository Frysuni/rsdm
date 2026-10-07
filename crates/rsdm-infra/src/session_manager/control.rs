//! User-bus requests; lifecycle decisions are serialized by the coordinator.

use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};
use zbus::{Connection, message::Header, object_server::ResponseDispatchNotifier, zvariant::Type};

use super::SessionError;

pub(super) const BUS_NAME: &str = "org.rsdm.Session1";
pub(super) const OBJECT_PATH: &str = "/org/rsdm/Session1";

#[path = "control_connection.rs"]
mod connection;
pub(super) use connection::Server as ControlServer;

#[path = "control_payload.rs"]
mod payload;
pub(super) use payload::{validate_finalize, validate_launch};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(super) struct LaunchRequest {
    pub generation: String,
    pub argv: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub directory: String,
    pub timeout_secs: u64,
    pub on_timeout: String,
    pub method: String,
    pub quit_command: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SessionStatus {
    pub generation: String,
    pub login_session_id: String,
    pub desktop_entry_id: String,
    pub provider: String,
    pub phase: String,
    pub xsmp_available: bool,
    pub apps: Vec<(String, String, u64, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct StopOutcome {
    pub result: String,
    pub forced_units: Vec<String>,
    pub message: String,
}

pub(super) type Reply<T> = async_channel::Sender<Result<T, String>>;

pub(super) enum Request {
    Launch(LaunchRequest, Reply<String>),
    Finalize { generation: String, environment: Vec<(String, String)>, reply: Reply<()> },
    Stop { generation: String, action: String, reply: Reply<StopOutcome> },
    Cancel { generation: String, reply: Reply<()> },
    Status(Reply<SessionStatus>),
    XsmpPrepare { generation: String, units: Vec<String>, cancellable: bool, reply: Reply<Vec<String>> },
    StopReplySent,
}

#[derive(Clone)]
pub(super) struct Endpoint {
    pub requests: Sender<Request>,
    pub uid: u32,
}

impl Endpoint {
    async fn request<T: Send + 'static>(
        &self, make_request: impl FnOnce(Reply<T>) -> Request + Send,
    ) -> zbus::fdo::Result<T> {
        let (reply, result) = async_channel::bounded(1);
        self.requests.send(make_request(reply))
            .map_err(|_| zbus::fdo::Error::Failed("session coordinator stopped".into()))?;
        result.recv().await.map_err(|_| zbus::fdo::Error::Failed("session request was interrupted".into()))?
            .map_err(zbus::fdo::Error::Failed)
    }

    async fn authorize(&self, connection: &Connection, header: &Header<'_>) -> zbus::fdo::Result<()> {
        let uid = if connection.is_bus() {
            let sender = header.sender().ok_or_else(|| zbus::fdo::Error::AccessDenied("caller has no bus identity".into()))?;
            let bus = zbus::Proxy::new(connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
            bus.call::<_, _, u32>("GetConnectionUnixUser", &(sender,)).await?
        } else {
            connection.peer_creds().await
                .map_err(|error| zbus::fdo::Error::AccessDenied(error.to_string()))?
                .unix_user_id().ok_or_else(|| zbus::fdo::Error::AccessDenied("caller has no Unix credentials".into()))?
        };
        if uid != self.uid {
            return Err(zbus::fdo::Error::AccessDenied("session belongs to another user".into()));
        }
        Ok(())
    }
}

#[zbus_macros::interface(name = "org.rsdm.Session1")]
impl Endpoint {
    async fn launch(
        &self, request: LaunchRequest,
        #[zbus(connection)] connection: &Connection, #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<String> {
        self.authorize(connection, &header).await?;
        validate_launch(&request).map_err(zbus::fdo::Error::InvalidArgs)?;
        self.request(|reply| Request::Launch(request, reply)).await
    }

    async fn finalize(
        &self, generation: String, environment: Vec<(String, String)>,
        #[zbus(connection)] connection: &Connection, #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        self.authorize(connection, &header).await?;
        validate_finalize(&generation, &environment).map_err(zbus::fdo::Error::InvalidArgs)?;
        self.request(|reply| Request::Finalize { generation, environment, reply }).await
    }

    async fn stop(
        &self, generation: String, action: String,
        #[zbus(connection)] connection: &Connection, #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<ResponseDispatchNotifier<StopOutcome>> {
        self.authorize(connection, &header).await?;
        payload::validate_stop(&generation, &action).map_err(zbus::fdo::Error::InvalidArgs)?;
        let outcome = self.request(|reply| Request::Stop { generation, action, reply }).await?;
        let (response, sent) = ResponseDispatchNotifier::new(outcome);
        let requests = self.requests.clone();
        connection.executor().spawn(async move {
            sent.await;
            let _ = requests.send(Request::StopReplySent);
        }, "rsdm session stop response").detach();
        Ok(response)
    }

    async fn cancel(
        &self, generation: String,
        #[zbus(connection)] connection: &Connection, #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        self.authorize(connection, &header).await?;
        payload::validate_generation(&generation).map_err(zbus::fdo::Error::InvalidArgs)?;
        self.request(|reply| Request::Cancel { generation, reply }).await
    }

    async fn status(
        &self, #[zbus(connection)] connection: &Connection, #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<SessionStatus> {
        self.authorize(connection, &header).await?;
        self.request(Request::Status).await
    }

    async fn xsmp_prepare(
        &self, generation: String, units: Vec<String>, cancellable: bool,
        #[zbus(connection)] connection: &Connection, #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<Vec<String>> {
        self.authorize(connection, &header).await?;
        payload::validate_xsmp(&generation, &units).map_err(zbus::fdo::Error::InvalidArgs)?;
        self.request(|reply| Request::XsmpPrepare { generation, units, cancellable, reply }).await
    }
}

pub(super) fn proxy(connection: &zbus::blocking::Connection) -> Result<zbus::blocking::Proxy<'_>, SessionError> {
    Ok(zbus::blocking::Proxy::new(connection, BUS_NAME, OBJECT_PATH, BUS_NAME)?)
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;

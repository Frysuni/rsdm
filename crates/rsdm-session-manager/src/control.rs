//! User-bus requests; lifecycle decisions are serialized by the coordinator.

use std::sync::{Arc, mpsc::{self, Receiver, SyncSender, TrySendError}};

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

#[path = "control_admission.rs"]
mod admission;
use admission::{Admission, Class, Permit};
pub(super) use admission::Reply;

pub(super) enum Request {
    Launch(LaunchRequest, Reply<String>),
    Finalize { generation: String, environment: Vec<(String, String)>, reply: Reply<()> },
    Stop { generation: String, action: String, reply: Reply<StopOutcome> },
    Cancel { generation: String, reply: Reply<()> },
    Status(Reply<SessionStatus>),
    XsmpPrepare { generation: String, units: Vec<String>, cancellable: bool, reply: Reply<Vec<String>> },
    StopReplySent(Arc<Permit>),
}

#[derive(Clone)]
pub(super) struct Endpoint {
    requests: SyncSender<Request>,
    uid: u32,
    admission: Admission,
}

impl Endpoint {
    pub fn channel(uid: u32) -> (Self, Receiver<Request>) {
        let (requests, received) = mpsc::sync_channel(admission::QUEUE_CAPACITY);
        (Self { requests, uid, admission: Admission::default() }, received)
    }

    async fn request<T: Send + 'static>(
        &self, permit: Arc<Permit>, make_request: impl FnOnce(Reply<T>) -> Request + Send,
    ) -> zbus::fdo::Result<T> {
        let (sender, result) = async_channel::bounded(1);
        let reply = Reply::new(sender, permit.clone());
        self.requests.try_send(make_request(reply)).map_err(|error| match error {
            TrySendError::Full(_) => zbus::fdo::Error::LimitsExceeded("session coordinator queue is full".into()),
            TrySendError::Disconnected(_) => zbus::fdo::Error::Failed("session coordinator stopped".into()),
        })?;
        let outcome = result.recv().await
            .map_err(|_| zbus::fdo::Error::Failed("session request was interrupted".into()))?;
        drop(permit);
        outcome.map_err(zbus::fdo::Error::Failed)
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
        let permit = self.admission.acquire(Class::Regular)?;
        self.authorize(connection, &header).await?;
        validate_launch(&request).map_err(zbus::fdo::Error::InvalidArgs)?;
        self.request(permit, |reply| Request::Launch(request, reply)).await
    }

    async fn finalize(
        &self, generation: String, environment: Vec<(String, String)>,
        #[zbus(connection)] connection: &Connection, #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let permit = self.admission.acquire(Class::Regular)?;
        self.authorize(connection, &header).await?;
        validate_finalize(&generation, &environment).map_err(zbus::fdo::Error::InvalidArgs)?;
        self.request(permit, |reply| Request::Finalize { generation, environment, reply }).await
    }

    async fn stop(
        &self, generation: String, action: String,
        #[zbus(connection)] connection: &Connection, #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<ResponseDispatchNotifier<StopOutcome>> {
        let permit = self.admission.acquire(Class::Stop)?;
        self.authorize(connection, &header).await?;
        payload::validate_stop(&generation, &action).map_err(zbus::fdo::Error::InvalidArgs)?;
        let outcome = self.request(permit.clone(), |reply| Request::Stop { generation, action, reply }).await?;
        let (response, sent) = ResponseDispatchNotifier::new(outcome);
        let requests = self.requests.clone();
        connection.executor().spawn(async move {
            sent.await;
            let _ = requests.try_send(Request::StopReplySent(permit));
        }, "rsdm session stop response").detach();
        Ok(response)
    }

    async fn cancel(
        &self, generation: String,
        #[zbus(connection)] connection: &Connection, #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let permit = self.admission.acquire(Class::Cancel)?;
        self.authorize(connection, &header).await?;
        payload::validate_generation(&generation).map_err(zbus::fdo::Error::InvalidArgs)?;
        self.request(permit, |reply| Request::Cancel { generation, reply }).await
    }

    async fn status(
        &self, #[zbus(connection)] connection: &Connection, #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<SessionStatus> {
        let permit = self.admission.acquire(Class::Regular)?;
        self.authorize(connection, &header).await?;
        self.request(permit, Request::Status).await
    }

    async fn xsmp_prepare(
        &self, generation: String, units: Vec<String>, cancellable: bool,
        #[zbus(connection)] connection: &Connection, #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<Vec<String>> {
        let permit = self.admission.acquire(Class::Xsmp)?;
        self.authorize(connection, &header).await?;
        payload::validate_xsmp(&generation, &units).map_err(zbus::fdo::Error::InvalidArgs)?;
        self.request(permit, |reply| Request::XsmpPrepare { generation, units, cancellable, reply }).await
    }
}

pub(super) fn proxy(connection: &zbus::blocking::Connection) -> Result<zbus::blocking::Proxy<'_>, SessionError> {
    Ok(zbus::blocking::Proxy::new(connection, BUS_NAME, OBJECT_PATH, BUS_NAME)?)
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;

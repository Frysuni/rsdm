//! Shutdown writes share the IPC budget and require a current session and peer.

use std::time::Duration;

use rsdm_core::domain::SessionPhase;
use zbus::{Connection, Proxy, OwnedObjectPath};

use super::{ffi, peer::Peer, protocol::Phase};
use crate::{SessionError, app_stop::ShutdownControl, bus::UserManager, processes::monotonic_usec, runtime::{Runtime, SessionRecord}};

#[derive(Clone, Copy)]
pub(super) enum Request { Save, Phase2, Die, Interact }

pub(super) async fn send(manager: &UserManager, runtime: &Runtime, peer: &mut Peer,
    transaction_id: Option<&str>, control: Option<&ShutdownControl>, request: Request) -> Result<bool, SessionError> {
    if control.is_some_and(ShutdownControl::cancelled) { return Ok(false); }
    let connection = manager.connection();
    let mut die_claim = None;
    let mut sent = false;

    let result = manager.request(async {
        let records = runtime.clone();
        let unit = peer.unit.clone();
        let (mut session, app) = crate::runtime::blocking(move || Ok((records.session()?, records.app(&unit)?))).await
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        if app.invocation_id != peer.invocation || !active(&session, transaction_id) {
            cancel(peer);
            return Ok(false);
        }
        if matches!(request, Request::Die) && !peer.die_claimed.get() {
            let Some(current) = crate::app_dispatch::claim_xsmp_die(runtime, &app, transaction_id, &manager.deadline).await
                .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?
            else { cancel(peer); return Ok(false); };
            session = current;
            peer.die_claimed.set(true);
            die_claim = Some(app.clone());
        } else if matches!(request, Request::Die) && !app.dispatch.as_ref().is_some_and(|dispatch|
            dispatch.transaction_id.as_deref() == transaction_id && dispatch.invocation_id == peer.invocation
            && dispatch.method == crate::app_dispatch::Method::Xsmp && dispatch.state == crate::app_dispatch::State::Sent
            && matches!(dispatch.xsmp_die_state, Some(crate::app_dispatch::State::Intent | crate::app_dispatch::State::Sent)))
        {
            cancel(peer);
            return Ok(false);
        }
        if !current_peer(&connection, &app, peer).await?
            || !peer.alive().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?
        {
            cancel(peer);
            return Ok(false);
        }
        sent = dispatch(manager, peer, &session, control, request)?;
        Ok(sent)
    }).await;

    if let Some(app) = die_claim {
        let state = if sent { crate::app_dispatch::State::Sent } else { crate::app_dispatch::State::NotSent };
        crate::app_dispatch::finish_xsmp_die(runtime, &app, transaction_id, state, &manager.deadline).await?;
        if !sent { peer.die_claimed.set(false); }
    }
    result.map_err(Into::into)
}

fn active(session: &SessionRecord, transaction_id: Option<&str>) -> bool {
    !session.disabled && session.phase != SessionPhase::Closed && session.pending_transaction_id() == transaction_id
}

async fn current_peer(connection: &Connection, app: &crate::runtime::AppRecord, peer: &Peer) -> zbus::Result<bool> {
    if app.migrated_scope.is_none() {
        return Ok(current_process(connection, peer).await? && current_invocation(connection, peer).await?);
    }
    let manager = Proxy::new(connection, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager").await?;
    crate::app_ownership::current(&manager, app, peer.pid, &peer.process).await
}

async fn current_process(connection: &Connection, peer: &Peer) -> zbus::Result<bool> {
    if !peer.alive().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))? { return Ok(false); }
    let manager = Proxy::new(connection, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager").await?;
    let path: OwnedObjectPath = manager.call("GetUnit", &(&peer.unit,)).await?;
    let service = zbus::proxy::Builder::<Proxy<'_>>::new(connection).destination("org.freedesktop.systemd1").path(path)
        .interface("org.freedesktop.systemd1.Service").cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    let control: u32 = service.get_property("ControlPID").await?;
    let processes: Vec<(String, u32, String)> = manager.call("GetUnitProcesses", &(&peer.unit,)).await?;
    Ok(peer.pid != control && processes.iter().any(|(_, pid, _)| *pid == peer.pid))
}

async fn current_invocation(connection: &Connection, peer: &Peer) -> zbus::Result<bool> {
    let manager = Proxy::new(connection, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager").await?;
    let path: OwnedObjectPath = manager.call("GetUnitByPID", &(peer.pid,)).await?;
    let unit = zbus::proxy::Builder::<Proxy<'_>>::new(connection).destination("org.freedesktop.systemd1").path(path)
        .interface("org.freedesktop.systemd1.Unit").cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    let name: String = unit.get_property("Id").await?;
    let invocation: Vec<u8> = unit.get_property("InvocationID").await?;
    Ok(name == peer.unit && !peer.invocation.is_empty() && peer.invocation.iter().any(|byte| *byte != 0) && invocation == peer.invocation)
}

fn dispatch(manager: &UserManager, peer: &mut Peer, session: &SessionRecord,
    control: Option<&ShutdownControl>, request: Request) -> zbus::Result<bool> {
    manager.deadline.remaining(Duration::MAX)?;
    let deadline = session.shutdown_deadline_usec.map_or(peer.deadline, |hard| peer.deadline.min(hard));
    if monotonic_usec().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))? >= deadline
        || control.is_some_and(ShutdownControl::cancelled)
    { return Ok(false); }
    manager.actions.check().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;

    // The worker owns this live libSM connection and holds one request slot.
    unsafe {
        match request {
            Request::Save => { peer.phase = Phase::Saving; ffi::SmsSaveYourself(peer.sms, 2, 1, 2, 0); }
            Request::Phase2 => { peer.phase = Phase::Phase2Saving; ffi::SmsSaveYourselfPhase2(peer.sms); }
            Request::Die => { peer.phase = Phase::Dying; ffi::SmsDie(peer.sms); }
            Request::Interact => { peer.interaction_requested = false; peer.interacting = true; ffi::SmsInteract(peer.sms); }
        }
    }
    Ok(true)
}

pub(super) fn cancel(peer: &mut Peer) {
    if peer.selected && !peer.sms.is_null() {
        match peer.phase {
            Phase::Saving | Phase::Phase2Waiting | Phase::Phase2Saving | Phase::TimedOut => {
                unsafe { ffi::SmsShutdownCancelled(peer.sms); }
                peer.phase = Phase::Cancelling;
            }
            Phase::Saved | Phase::Failed => {
                unsafe { ffi::SmsShutdownCancelled(peer.sms); ffi::SmsSaveComplete(peer.sms); }
                peer.phase = Phase::Idle;
            }
            _ => {},
        }
    }
    peer.selected = false;
    peer.interaction_requested = false;
    peer.interacting = false;
    peer.cancellation_requested = false;
}

//! libSM callbacks contain panics and only mutate their admitted connection.

use std::{cell::Cell, panic::{AssertUnwindSafe, catch_unwind}, ptr};

use libc::{c_char, c_int, c_ulong};

use super::{ffi::{self, Callback, Data, SmsConn}, peer::Peer, protocol::Phase};

pub(super) struct Admission {
    pub peer: Cell<*mut Peer>,
}

pub(super) unsafe extern "C" fn new_client(
    sms: SmsConn, data: Data, mask: *mut c_ulong, callbacks: *mut ffi::Callbacks, _failure: *mut *mut c_char,
) -> c_int {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Admission lives until all connections have been closed; the
        // server sets its peer only for the current IceProcessMessages call.
        let peer = unsafe { &*(data.cast::<Admission>()) }.peer.get();
        if peer.is_null() || unsafe { (*peer).ice != ffi::SmsGetIceConnection(sms) || !(*peer).sms.is_null() } {
            return 0;
        }
        unsafe {
            (*peer).sms = sms;
            *mask = (1 << 10) - 1;
            *callbacks = table(peer.cast());
        }
        1
    })).unwrap_or(0)
}

fn table(data: Data) -> ffi::Callbacks {
    ffi::Callbacks {
        register: Callback { callback: register, data },
        interact_request: Callback { callback: interact_request, data },
        interact_done: Callback { callback: interact_done, data },
        save_request: Callback { callback: save_request, data },
        phase2: Callback { callback: phase2, data },
        save_done: Callback { callback: save_done, data },
        close: Callback { callback: close, data },
        set_properties: Callback { callback: set_properties, data },
        delete_properties: Callback { callback: delete_properties, data },
        get_properties: Callback { callback: get_properties, data },
    }
}

fn with_peer<T: Default>(data: Data, callback: impl FnOnce(&mut Peer) -> T) -> T {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: callback data points to a boxed Peer kept alive for the entire
        // ICE dispatch. No Rust borrow is held across that dispatch.
        callback(unsafe { &mut *data.cast::<Peer>() })
    }));
    match result {
        Ok(value) => value,
        Err(_) => {
            unsafe { (*data.cast::<Peer>()).phase = Phase::Failed; }
            T::default()
        }
    }
}

unsafe extern "C" fn register(sms: SmsConn, data: Data, previous: *mut c_char) -> c_int {
    if !previous.is_null() {
        // No restored client registry exists. Reject previous IDs and let libSM
        // re-register the connection with a fresh, library-generated ID.
        unsafe { libc::free(previous.cast()); }
        return 0;
    }
    with_peer(data, |peer| {
        let id = unsafe { ffi::SmsGenerateClientID(sms) };
        if id.is_null() { peer.phase = Phase::Failed; return 0; }
        let registered = unsafe { ffi::SmsRegisterClientReply(sms, id) };
        unsafe { libc::free(id.cast()); }
        if registered != 0 {
            peer.phase = Phase::InitialSave;
            unsafe { ffi::SmsSaveYourself(sms, 1, 0, 0, 0); }
        }
        registered
    })
}

unsafe extern "C" fn interact_request(_sms: SmsConn, data: Data, _dialog: c_int) {
    with_peer(data, |peer| {
        if peer.phase.saving() { peer.interaction_requested = true; }
    });
}

unsafe extern "C" fn interact_done(_sms: SmsConn, data: Data, cancel: c_int) {
    with_peer(data, |peer| {
        peer.interacting = false;
        peer.cancellation_requested |= cancel != 0;
    });
}

unsafe extern "C" fn save_request(
    _sms: SmsConn, data: Data, _kind: c_int, shutdown: c_int, _style: c_int, _fast: c_int, _global: c_int,
) {
    with_peer(data, |peer| {
        // XSMP clients may request a local save, but cannot authorize logout or
        // power for the whole login session through this protocol connection.
        if shutdown == 0 && peer.phase == Phase::Idle { peer.local_save_requested = true; }
    });
}

unsafe extern "C" fn phase2(sms: SmsConn, data: Data) {
    with_peer(data, |peer| {
        if peer.phase == Phase::Saving { peer.phase = Phase::Phase2Waiting; }
        else if peer.phase == Phase::InitialSave { unsafe { ffi::SmsSaveYourselfPhase2(sms); } }
    });
}

unsafe extern "C" fn save_done(_sms: SmsConn, data: Data, success: c_int) {
    with_peer(data, |peer| peer.phase.save_done(success != 0));
}

unsafe extern "C" fn close(sms: SmsConn, data: Data, count: c_int, reasons: *mut *mut c_char) {
    // SAFETY: libSM transferred the reasons and this connection's cleanup.
    unsafe { ffi::SmFreeReasons(count, reasons); ffi::SmsCleanUp(sms); }
    with_peer(data, |peer| { peer.sms = ptr::null_mut(); peer.phase = Phase::Closed; });
}

unsafe extern "C" fn set_properties(_sms: SmsConn, data: Data, count: c_int, properties: *mut *mut ffi::Property) {
    with_peer(data, |peer| unsafe { peer.properties.set(count, properties); });
}

unsafe extern "C" fn delete_properties(_sms: SmsConn, data: Data, count: c_int, names: *mut *mut c_char) {
    with_peer(data, |peer| unsafe { peer.properties.delete(count, names); });
}

unsafe extern "C" fn get_properties(sms: SmsConn, data: Data) {
    with_peer(data, |peer| unsafe { peer.properties.reply(sms); });
}

pub(super) unsafe extern "C" fn protocol_error(
    _connection: Data, _swap: c_int, _opcode: c_int, _sequence: c_ulong, _class: c_int, _severity: c_int, _values: Data,
) {}

pub(super) unsafe extern "C" fn io_error(_connection: ffi::IceConn) {}

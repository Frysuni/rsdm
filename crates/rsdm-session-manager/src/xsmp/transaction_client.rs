//! A libSM client runs in a separate process from the tested server.

use std::{io::{Read, Write}, ptr, sync::atomic::{AtomicBool, Ordering}};

use libc::{c_char, c_int, c_ulong, c_void};

use super::super::super::ffi::{self, Callback, Data, IceConn};

type SmcConn = *mut c_void;
static DISCONNECTED: AtomicBool = AtomicBool::new(false);

#[repr(C)]
struct Callbacks {
    save: Callback<unsafe extern "C" fn(SmcConn, Data, c_int, c_int, c_int, c_int)>,
    die: Callback<unsafe extern "C" fn(SmcConn, Data)>,
    complete: Callback<unsafe extern "C" fn(SmcConn, Data)>,
    cancel: Callback<unsafe extern "C" fn(SmcConn, Data)>,
}

unsafe extern "C" {
    fn SmcOpenConnection(networks: *const c_char, context: Data, major: c_int, minor: c_int,
        mask: c_ulong, callbacks: *mut Callbacks, previous: *const c_char, id: *mut *mut c_char,
        error_length: c_int, error: *mut c_char) -> SmcConn;
    fn SmcGetIceConnection(connection: SmcConn) -> IceConn;
    fn SmcSaveYourselfDone(connection: SmcConn, success: c_int);
    fn SmcRequestSaveYourselfPhase2(connection: SmcConn, callback: unsafe extern "C" fn(SmcConn, Data), data: Data) -> c_int;
    fn SmcInteractRequest(connection: SmcConn, dialog: c_int, callback: unsafe extern "C" fn(SmcConn, Data), data: Data) -> c_int;
    fn SmcInteractDone(connection: SmcConn, cancel: c_int);
    fn SmcCloseConnection(connection: SmcConn, count: c_int, reasons: *mut *mut c_char) -> c_int;
}

#[derive(Default)]
struct State { mode: String, saves: usize, dies: usize, cancels: usize, phase2: usize, interactions: usize, ready: bool, request_failed: bool }

pub(super) fn run(mode: &str) {
    let mut state = State { mode: mode.into(), ..State::default() };
    let data = (&mut state as *mut State).cast();
    let mut callbacks = Callbacks {
        save: Callback { callback: save, data }, die: Callback { callback: die, data },
        complete: Callback { callback: complete, data }, cancel: Callback { callback: cancel, data },
    };
    let mut id = ptr::null_mut();
    let mut error = [0; 256];
    // SAFETY: callback data and outputs remain valid until the connection closes.
    let connection = unsafe {
        ffi::IceSetIOErrorHandler(disconnected);
        SmcOpenConnection(ptr::null(), ptr::null_mut(), 1, 0, 15, &mut callbacks, ptr::null(), &mut id, 256, error.as_mut_ptr())
    };
    unsafe { libc::free(id.cast()); }
    assert!(!connection.is_null(), "libSM client could not connect: {}", unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) }.to_string_lossy());
    let ice = unsafe { SmcGetIceConnection(connection) };
    while state.dies == 0 && !DISCONNECTED.load(Ordering::SeqCst) {
        if unsafe { ffi::IceProcessMessages(ice, ptr::null_mut(), ptr::null_mut()) } != 0 { break; }
    }
    unsafe { SmcCloseConnection(connection, 0, ptr::null_mut()); }
    let followup = mode.starts_with("followup-");
    let owned = followup && mode.ends_with("-owned");
    assert_eq!(state.saves, usize::from(followup || matches!(mode, "forced" | "after-send" | "phase2" | "interact")), "unexpected shutdown SaveYourself on the wire");
    assert_eq!(state.dies, usize::from(owned || mode == "forced"), "unexpected Die on the wire");
    assert_eq!(state.cancels, usize::from((followup && !owned) || matches!(mode, "after-send" | "phase2" | "interact")), "unexpected ShutdownCancelled on the wire");
    assert_eq!(state.phase2, usize::from(owned && mode.contains("-phase2-")), "unexpected phase 2 on the wire");
    assert_eq!(state.interactions, usize::from(owned && mode.contains("-interact-")), "unexpected interaction on the wire");
    assert!(!state.request_failed, "libSM did not send the client's phase 2 or interaction request");
}

unsafe extern "C" fn save(connection: SmcConn, data: Data, _kind: c_int, shutdown: c_int, _style: c_int, _fast: c_int) {
    // SAFETY: the connection owns this live callback context for its lifetime.
    let state = unsafe { &mut *data.cast::<State>() };
    if shutdown != 0 {
        state.saves += 1;
        if state.mode.starts_with("followup-") || matches!(state.mode.as_str(), "after-send" | "phase2" | "interact") {
            println!("xsmp-client-saving");
            let _ = std::io::stdout().flush();
            let _ = std::io::stdin().read_exact(&mut [0_u8]);
        }
        if state.mode == "phase2" || state.mode.starts_with("followup-phase2-") {
            state.request_failed = unsafe { SmcRequestSaveYourselfPhase2(connection, phase2, data) } == 0;
            println!("xsmp-client-followup-requested");
            let _ = std::io::stdout().flush();
            return;
        }
        if state.mode == "interact" || state.mode.starts_with("followup-interact-") {
            state.request_failed = unsafe { SmcInteractRequest(connection, 1, interact, data) } == 0;
            println!("xsmp-client-followup-requested");
            let _ = std::io::stdout().flush();
            return;
        }
    }
    unsafe { SmcSaveYourselfDone(connection, 1); }
    if shutdown != 0 {
        println!("xsmp-client-followup-requested");
        let _ = std::io::stdout().flush();
    }
}

unsafe extern "C" fn die(_connection: SmcConn, data: Data) {
    unsafe { (*data.cast::<State>()).dies += 1; }
}

unsafe extern "C" fn complete(_connection: SmcConn, data: Data) {
    let state = unsafe { &mut *data.cast::<State>() };
    if !state.ready {
        state.ready = true;
        println!("xsmp-client-ready");
        let _ = std::io::stdout().flush();
    }
}

unsafe extern "C" fn cancel(connection: SmcConn, data: Data) {
    let state = unsafe { &mut *data.cast::<State>() };
    state.cancels += 1;
    if state.mode.contains("phase2") || state.mode.contains("interact") { unsafe { SmcSaveYourselfDone(connection, 1); } }
    println!("xsmp-client-cancelled");
    let _ = std::io::stdout().flush();
}

unsafe extern "C" fn phase2(connection: SmcConn, data: Data) {
    unsafe { (*data.cast::<State>()).phase2 += 1; SmcSaveYourselfDone(connection, 1); }
}

unsafe extern "C" fn interact(connection: SmcConn, data: Data) {
    unsafe { (*data.cast::<State>()).interactions += 1; SmcInteractDone(connection, 0); SmcSaveYourselfDone(connection, 1); }
}

unsafe extern "C" fn disconnected(_connection: IceConn) { DISCONNECTED.store(true, Ordering::SeqCst); }

//! The libSM/libICE server ABI. All calls stay on the server thread.

use libc::{c_char, c_int, c_ulong, c_ushort, c_void};

pub type IceConn = *mut c_void;
pub type ListenObj = *mut c_void;
pub type SmsConn = *mut c_void;
pub type Data = *mut c_void;

#[repr(C)]
pub struct Callback<F> {
    pub callback: F,
    pub data: Data,
}

#[repr(C)]
pub struct Callbacks {
    pub register: Callback<unsafe extern "C" fn(SmsConn, Data, *mut c_char) -> c_int>,
    pub interact_request: Callback<unsafe extern "C" fn(SmsConn, Data, c_int)>,
    pub interact_done: Callback<unsafe extern "C" fn(SmsConn, Data, c_int)>,
    pub save_request: Callback<unsafe extern "C" fn(SmsConn, Data, c_int, c_int, c_int, c_int, c_int)>,
    pub phase2: Callback<unsafe extern "C" fn(SmsConn, Data)>,
    pub save_done: Callback<unsafe extern "C" fn(SmsConn, Data, c_int)>,
    pub close: Callback<unsafe extern "C" fn(SmsConn, Data, c_int, *mut *mut c_char)>,
    pub set_properties: Callback<unsafe extern "C" fn(SmsConn, Data, c_int, *mut *mut Property)>,
    pub delete_properties: Callback<unsafe extern "C" fn(SmsConn, Data, c_int, *mut *mut c_char)>,
    pub get_properties: Callback<unsafe extern "C" fn(SmsConn, Data)>,
}

#[repr(C)]
pub struct PropertyValue {
    pub length: c_int,
    pub value: Data,
}

#[repr(C)]
pub struct Property {
    pub name: *mut c_char,
    pub kind: *mut c_char,
    pub count: c_int,
    pub values: *mut PropertyValue,
}

#[repr(C)]
pub struct AuthData {
    pub protocol: *mut c_char,
    pub network: *mut c_char,
    pub name: *mut c_char,
    pub length: c_ushort,
    pub data: *mut c_char,
}

pub type ErrorHandler = unsafe extern "C" fn(Data, c_int, c_int, c_ulong, c_int, c_int, Data);
pub type NewClient = unsafe extern "C" fn(SmsConn, Data, *mut c_ulong, *mut Callbacks, *mut *mut c_char) -> c_int;

unsafe extern "C" {
    pub fn SmsInitialize(vendor: *const c_char, release: *const c_char, new_client: NewClient,
        data: Data, host_auth: Option<unsafe extern "C" fn(*mut c_char) -> c_int>,
        error_length: c_int, error: *mut c_char) -> c_int;
    pub fn SmsGetIceConnection(connection: SmsConn) -> IceConn;
    pub fn SmsGenerateClientID(connection: SmsConn) -> *mut c_char;
    pub fn SmsRegisterClientReply(connection: SmsConn, id: *mut c_char) -> c_int;
    pub fn SmsSaveYourself(connection: SmsConn, save_type: c_int, shutdown: c_int, interact_style: c_int, fast: c_int);
    pub fn SmsSaveYourselfPhase2(connection: SmsConn);
    pub fn SmsInteract(connection: SmsConn);
    pub fn SmsSaveComplete(connection: SmsConn);
    pub fn SmsShutdownCancelled(connection: SmsConn);
    pub fn SmsDie(connection: SmsConn);
    pub fn SmsCleanUp(connection: SmsConn);
    pub fn SmsReturnProperties(connection: SmsConn, count: c_int, properties: *mut *mut Property);
    pub fn SmsSetErrorHandler(handler: ErrorHandler) -> Option<ErrorHandler>;
    pub fn SmFreeProperty(property: *mut Property);
    pub fn SmFreeReasons(count: c_int, reasons: *mut *mut c_char);

    pub fn IceListenForConnections(count: *mut c_int, objects: *mut *mut ListenObj, error_length: c_int, error: *mut c_char) -> c_int;
    pub fn IceGetListenConnectionNumber(object: ListenObj) -> c_int;
    pub fn IceGetListenConnectionString(object: ListenObj) -> *mut c_char;
    pub fn IceFreeListenObjs(count: c_int, objects: *mut ListenObj);
    pub fn IceSetHostBasedAuthProc(object: ListenObj, auth: Option<unsafe extern "C" fn(*mut c_char) -> c_int>);
    pub fn IceAcceptConnection(object: ListenObj, status: *mut c_int) -> IceConn;
    pub fn IceConnectionNumber(connection: IceConn) -> c_int;
    pub fn IceSetShutdownNegotiation(connection: IceConn, negotiate: c_int);
    pub fn IceCloseConnection(connection: IceConn) -> c_int;
    pub fn IceProcessMessages(connection: IceConn, reply_wait: Data, reply_ready: *mut c_int) -> c_int;
    pub fn IceSetPaAuthData(count: c_int, entries: *mut AuthData);
    pub fn IceSetErrorHandler(handler: ErrorHandler) -> Option<ErrorHandler>;
    pub fn IceSetIOErrorHandler(handler: unsafe extern "C" fn(IceConn)) -> Option<unsafe extern "C" fn(IceConn)>;
}

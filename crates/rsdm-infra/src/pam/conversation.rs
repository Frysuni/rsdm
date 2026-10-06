use std::os::raw::{c_char, c_int, c_void};
use zeroize::Zeroizing;

use super::ffi::PAM_SUCCESS;

const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;

#[repr(C)]
pub struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
pub struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

#[repr(C)]
pub struct PamConv {
    conv: Option<ConversationFn>,
    appdata_ptr: *mut c_void,
}

pub struct ConversationData {
    pub password: Option<Zeroizing<Vec<u8>>>,
}

impl PamConv {
    pub fn new(appdata_ptr: *mut c_void) -> Self {
        Self {
            conv: Some(conversation),
            appdata_ptr,
        }
    }
}

type ConversationFn = unsafe extern "C" fn(
    c_int,
    *mut *const PamMessage,
    *mut *mut PamResponse,
    *mut c_void,
) -> c_int;

pub unsafe extern "C" fn conversation(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int {
    if num_msg <= 0 || msg.is_null() || resp.is_null() || appdata_ptr.is_null() {
        return 1;
    }

    let count = num_msg as usize;
    let responses = allocate_responses(count);
    if responses.is_null() {
        return 1;
    }

    // SAFETY: PamHandle owns this context for the entire PAM transaction.
    let data = unsafe { &*appdata_ptr.cast::<ConversationData>() };
    let password = data.password.as_ref().map(|password| password.as_ptr().cast());
    for index in 0..count {
        if !answer_message(index, msg, responses, password) {
            // SAFETY: responses was allocated by this function. Slots through
            // index may contain strdup-owned strings and must be scrubbed before
            // freeing the array because one of them can be the password answer.
            unsafe { free_responses(responses, index + 1) };
            return 1;
        }
    }

    // SAFETY: resp is an out pointer supplied by PAM.
    unsafe {
        *resp = responses;
    }
    PAM_SUCCESS
}

fn allocate_responses(count: usize) -> *mut PamResponse {
    if count == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: calloc allocates a PAM-compatible response array initialized to zero.
    unsafe { libc::calloc(count, std::mem::size_of::<PamResponse>()) as *mut PamResponse }
}

fn answer_message(
    index: usize,
    msg: *mut *const PamMessage,
    responses: *mut PamResponse,
    password: Option<*const c_char>,
) -> bool {
    // SAFETY: PAM passes count valid message pointers and we allocated count responses.
    let message = unsafe { *msg.add(index) };
    if message.is_null() {
        return true;
    }
    // SAFETY: message and response slot are valid by PAM conversation contract.
    let style = unsafe { (*message).msg_style };
    let response = unsafe { responses.add(index) };

    if style == PAM_PROMPT_ECHO_OFF {
        let Some(password) = password else {
            return false;
        };
        // SAFETY: password points at a NUL-terminated buffer valid for the call.
        unsafe {
            (*response).resp = libc::strdup(password);
        }
        // SAFETY: response is a valid response slot.
        unsafe { !(*response).resp.is_null() }
    } else if style == PAM_PROMPT_ECHO_ON {
        // SAFETY: strdup creates a PAM-owned C string for the response.
        unsafe {
            (*response).resp = libc::strdup(c"".as_ptr());
        }
        // SAFETY: response is a valid response slot.
        unsafe { !(*response).resp.is_null() }
    } else {
        style == PAM_TEXT_INFO || style == PAM_ERROR_MSG
    }
}

unsafe fn free_responses(responses: *mut PamResponse, count: usize) {
    for index in 0..count {
        // SAFETY: caller guarantees responses points to at least count slots.
        let response = unsafe { responses.add(index) };
        // SAFETY: response points to a valid slot from the allocated array.
        let text = unsafe { (*response).resp };
        if !text.is_null() {
            // SAFETY: response strings are created by strdup, so they are
            // NUL-terminated and writable until freed.
            unsafe { zero_c_string(text) };
            // SAFETY: text was allocated by strdup and is owned by this array
            // on failure.
            unsafe { libc::free(text.cast()) };
        }
    }
    // SAFETY: responses was allocated by calloc in allocate_responses.
    unsafe { libc::free(responses.cast()) };
}

unsafe fn zero_c_string(text: *mut c_char) {
    // SAFETY: caller guarantees text points to a valid NUL-terminated C string.
    let len = unsafe { libc::strlen(text) };
    if len > 0 {
        // SAFETY: strdup allocations are writable for len bytes before the NUL.
        unsafe { std::ptr::write_bytes(text.cast::<u8>(), 0, len) };
    }
}

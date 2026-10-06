use std::{
    ffi::CString,
    os::raw::{c_char, c_int, c_void},
};
use zeroize::{Zeroize, Zeroizing};

use super::ffi::PAM_SUCCESS;

const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;
const PAM_CONV_ERR: c_int = 19;
const PAM_MAX_NUM_MSG: c_int = 32;

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
    pub username: CString,
    pub password_answered: bool,
    pub username_answered: bool,
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
    if resp.is_null() {
        return PAM_CONV_ERR;
    }
    // SAFETY: PAM supplies a writable response out pointer.
    unsafe { *resp = std::ptr::null_mut() };
    if !(1..=PAM_MAX_NUM_MSG).contains(&num_msg) || msg.is_null() || appdata_ptr.is_null() {
        return PAM_CONV_ERR;
    }

    let count = num_msg as usize;
    let responses = allocate_responses(count);
    if responses.is_null() {
        return PAM_CONV_ERR;
    }

    // SAFETY: PamHandle owns this context for the entire PAM transaction.
    let data = unsafe { &mut *appdata_ptr.cast::<ConversationData>() };
    for index in 0..count {
        if !answer_message(index, msg, responses, data) {
            // SAFETY: responses was allocated by this function. Slots through
            // index may contain strdup-owned strings and must be scrubbed before
            // freeing the array because one of them can be the password answer.
            unsafe { free_responses(responses, index + 1) };
            return PAM_CONV_ERR;
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
    data: &mut ConversationData,
) -> bool {
    // SAFETY: PAM passes count valid message pointers and we allocated count responses.
    let message = unsafe { *msg.add(index) };
    if message.is_null() {
        return false;
    }
    // SAFETY: message and response slot are valid by PAM conversation contract.
    let style = unsafe { (*message).msg_style };
    let response = unsafe { responses.add(index) };

    if style == PAM_PROMPT_ECHO_OFF {
        if data.password_answered {
            return false;
        }
        let Some(password) = data.password.as_ref() else {
            return false;
        };
        data.password_answered = true;
        // SAFETY: password points at a NUL-terminated buffer valid for the call.
        unsafe {
            (*response).resp = libc::strdup(password.as_ptr().cast());
        }
        // SAFETY: response is a valid response slot.
        unsafe { !(*response).resp.is_null() }
    } else if style == PAM_PROMPT_ECHO_ON {
        if data.username_answered {
            return false;
        }
        data.username_answered = true;
        // SAFETY: strdup creates a PAM-owned C string for the response.
        unsafe {
            (*response).resp = libc::strdup(data.username.as_ptr());
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
        unsafe { std::slice::from_raw_parts_mut(text.cast::<u8>(), len) }.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> ConversationData {
        ConversationData {
            password: Some(Zeroizing::new(b"secret\0".to_vec())),
            username: CString::new("alice").unwrap(),
            password_answered: false,
            username_answered: false,
        }
    }

    fn ask(data: &mut ConversationData, style: c_int) -> (c_int, *mut PamResponse) {
        let message = PamMessage { msg_style: style, msg: c"prompt".as_ptr() };
        let mut messages = [&message as *const PamMessage];
        let mut responses = std::ptr::null_mut();
        // SAFETY: all pointers reference live objects for the duration of the callback.
        let status = unsafe {
            conversation(1, messages.as_mut_ptr(), &mut responses, (data as *mut ConversationData).cast())
        };
        (status, responses)
    }

    #[test]
    fn password_is_not_reused_for_a_later_challenge() {
        let mut data = data();
        let (status, responses) = ask(&mut data, PAM_PROMPT_ECHO_OFF);
        assert_eq!(status, PAM_SUCCESS);
        // SAFETY: a successful callback returns one allocated response.
        unsafe { free_responses(responses, 1) };

        let (status, responses) = ask(&mut data, PAM_PROMPT_ECHO_OFF);
        assert_eq!(status, PAM_CONV_ERR);
        assert!(responses.is_null());
    }

    #[test]
    fn echoed_username_prompt_receives_the_account_name() {
        let mut data = data();
        let (status, responses) = ask(&mut data, PAM_PROMPT_ECHO_ON);
        assert_eq!(status, PAM_SUCCESS);
        // SAFETY: a successful echoed prompt returns one NUL-terminated response.
        unsafe {
            assert_eq!(std::ffi::CStr::from_ptr((*responses).resp), c"alice");
            free_responses(responses, 1);
        }
    }
}

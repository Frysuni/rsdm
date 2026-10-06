use std::{
    ffi::CString,
    os::raw::{c_char, c_int, c_void},
};

use rsdm_core::{
    domain::PasswordSecret,
    ports::{AuthConversation, AuthMessage, AuthMessageStyle, MAX_PASSWORD_BYTES},
};

use zeroize::{Zeroize, Zeroizing};

use super::bindings::PAM_SUCCESS;

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
    pub conversation: Option<Box<dyn AuthConversation>>,
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

    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        answer_messages(num_msg as usize, msg, resp, appdata_ptr)
    }))
    .unwrap_or(PAM_CONV_ERR)
}

fn answer_messages(
    count: usize,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int {
    let Some(responses) = ResponseBuffer::new(count) else {
        return PAM_CONV_ERR;
    };

    // SAFETY: PamHandle owns this context for the entire PAM transaction.
    let data = unsafe { &mut *appdata_ptr.cast::<ConversationData>() };
    for index in 0..count {
        if !answer_message(index, msg, responses.ptr, data) {
            return PAM_CONV_ERR;
        }
    }

    // SAFETY: resp is an out pointer supplied by PAM.
    unsafe {
        *resp = responses.ptr;
    }
    std::mem::forget(responses);
    PAM_SUCCESS
}

struct ResponseBuffer {
    ptr: *mut PamResponse,
    count: usize,
}

impl ResponseBuffer {
    fn new(count: usize) -> Option<Self> {
        // SAFETY: count is bounded by PAM_MAX_NUM_MSG; calloc zeroes every slot.
        let ptr: *mut PamResponse =
            unsafe { libc::calloc(count, std::mem::size_of::<PamResponse>()).cast() };
        if ptr.is_null() {
            return None;
        }
        Some(Self { ptr, count })
    }
}

impl Drop for ResponseBuffer {
    fn drop(&mut self) {
        // SAFETY: this buffer owns all calloc slots and any answers stored in them.
        unsafe { free_responses(self.ptr, self.count) };
    }
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

    if style == PAM_PROMPT_ECHO_OFF && !data.password_answered {
        data.password_answered = true;
        if let Some(password) = data.password.take() {
            // SAFETY: the password is NUL-terminated; strdup transfers a copy to PAM.
            unsafe { (*response).resp = libc::strdup(password.as_ptr().cast()) };
            return unsafe { !(*response).resp.is_null() };
        }
    }
    if style == PAM_PROMPT_ECHO_ON && data.conversation.is_none() && !data.username_answered {
        data.username_answered = true;
        // SAFETY: username is a CString and the response slot is valid.
        unsafe { (*response).resp = libc::strdup(data.username.as_ptr()) };
        return unsafe { !(*response).resp.is_null() };
    }
    interactive_answer(message, data, response)
}

fn interactive_answer(
    message: *const PamMessage,
    data: &mut ConversationData,
    response: *mut PamResponse,
) -> bool {
    // SAFETY: answer_message checked that the PAM message pointer is non-null.
    let (style, text) = unsafe { ((*message).msg_style, (*message).msg) };
    let style = match style {
        PAM_PROMPT_ECHO_OFF => AuthMessageStyle::Secret,
        PAM_PROMPT_ECHO_ON => AuthMessageStyle::Visible,
        PAM_TEXT_INFO => AuthMessageStyle::Info,
        PAM_ERROR_MSG => AuthMessageStyle::Error,
        _ => return false,
    };
    let Some(conversation) = data.conversation.as_mut() else {
        return matches!(style, AuthMessageStyle::Info | AuthMessageStyle::Error);
    };
    if text.is_null() {
        return false;
    }
    // SAFETY: PAM message text is NUL-terminated and valid during the callback.
    let text = unsafe { std::ffi::CStr::from_ptr(text) };
    if text.to_bytes().len() > MAX_PASSWORD_BYTES {
        return false;
    }
    let message = AuthMessage {
        style,
        text: text.to_string_lossy().into_owned(),
    };
    let Ok(answer) = conversation.respond(message) else {
        return false;
    };
    if matches!(style, AuthMessageStyle::Info | AuthMessageStyle::Error) {
        return true;
    }
    copy_answer(answer, response)
}

fn copy_answer(answer: Option<PasswordSecret>, response: *mut PamResponse) -> bool {
    let Some(answer) = answer else {
        return false;
    };
    let bytes = answer.expose_secret().as_bytes();
    if bytes.len() > MAX_PASSWORD_BYTES || bytes.contains(&0) {
        return false;
    }
    let mut bytes = Zeroizing::new(bytes.to_vec());
    bytes.push(0);
    // SAFETY: response is a valid calloc slot and bytes is NUL-terminated.
    unsafe { (*response).resp = libc::strdup(bytes.as_ptr().cast()) };
    unsafe { !(*response).resp.is_null() }
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
    // SAFETY: responses was allocated by calloc in ResponseBuffer::new.
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
#[path = "conversation_tests.rs"]
mod tests;

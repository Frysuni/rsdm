//! Linux-PAM ABI declarations.

use std::os::raw::{c_char, c_int, c_void};

use super::conversation::PamConv;

pub(super) const PAM_SUCCESS: c_int = 0;
pub(super) const PAM_ESTABLISH_CRED: c_int = 0x2;
pub(super) const PAM_DELETE_CRED: c_int = 0x4;
pub(super) const PAM_TTY: c_int = 3;
pub(super) const PAM_USER: c_int = 2;
pub(super) const PAM_FAIL_DELAY: c_int = 10;

#[repr(C)]
pub struct PamHandleRaw {
    _private: [u8; 0],
}

#[link(name = "pam")]
unsafe extern "C" {
    pub(super) fn pam_start(
        service_name: *const c_char,
        user: *const c_char,
        pam_conversation: *const PamConv,
        pamh: *mut *mut PamHandleRaw,
    ) -> c_int;
    pub(super) fn pam_end(pamh: *mut PamHandleRaw, pam_status: c_int) -> c_int;
    pub(super) fn pam_authenticate(pamh: *mut PamHandleRaw, flags: c_int) -> c_int;
    pub(super) fn pam_acct_mgmt(pamh: *mut PamHandleRaw, flags: c_int) -> c_int;
    pub(super) fn pam_setcred(pamh: *mut PamHandleRaw, flags: c_int) -> c_int;
    pub(super) fn pam_open_session(pamh: *mut PamHandleRaw, flags: c_int) -> c_int;
    pub(super) fn pam_close_session(pamh: *mut PamHandleRaw, flags: c_int) -> c_int;
    pub(super) fn pam_getenvlist(pamh: *mut PamHandleRaw) -> *mut *mut c_char;
    pub(super) fn pam_strerror(pamh: *mut PamHandleRaw, errnum: c_int) -> *const c_char;
    pub(super) fn pam_set_item(pamh: *mut PamHandleRaw, item_type: c_int, item: *const c_void) -> c_int;
    pub(super) fn pam_get_item(pamh: *const PamHandleRaw, item_type: c_int, item: *mut *const c_void) -> c_int;
    pub(super) fn pam_putenv(pamh: *mut PamHandleRaw, name_value: *const c_char) -> c_int;
}

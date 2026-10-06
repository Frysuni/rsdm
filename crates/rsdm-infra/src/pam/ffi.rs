use std::{
    ffi::{CStr, CString},
    os::raw::{c_char, c_int, c_void},
    ptr,
};

use rsdm_core::ports::AuthError;
use zeroize::Zeroizing;

use super::conversation::{ConversationData, PamConv};

pub const PAM_SUCCESS: c_int = 0;
const PAM_ESTABLISH_CRED: c_int = 0x2;
const PAM_DELETE_CRED: c_int = 0x4;
const PAM_TTY: c_int = 3;
const PAM_FAIL_DELAY: c_int = 10;

#[repr(C)]
pub struct PamHandleRaw {
    _private: [u8; 0],
}

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(
        service_name: *const c_char,
        user: *const c_char,
        pam_conversation: *const PamConv,
        pamh: *mut *mut PamHandleRaw,
    ) -> c_int;
    fn pam_end(pamh: *mut PamHandleRaw, pam_status: c_int) -> c_int;
    fn pam_authenticate(pamh: *mut PamHandleRaw, flags: c_int) -> c_int;
    fn pam_acct_mgmt(pamh: *mut PamHandleRaw, flags: c_int) -> c_int;
    fn pam_setcred(pamh: *mut PamHandleRaw, flags: c_int) -> c_int;
    fn pam_open_session(pamh: *mut PamHandleRaw, flags: c_int) -> c_int;
    fn pam_close_session(pamh: *mut PamHandleRaw, flags: c_int) -> c_int;
    fn pam_getenvlist(pamh: *mut PamHandleRaw) -> *mut *mut c_char;
    fn pam_strerror(pamh: *mut PamHandleRaw, errnum: c_int) -> *const c_char;
    fn pam_set_item(pamh: *mut PamHandleRaw, item_type: c_int, item: *const c_void) -> c_int;
    fn pam_putenv(pamh: *mut PamHandleRaw, name_value: *const c_char) -> c_int;
}

pub struct PamHandle {
    raw: *mut PamHandleRaw,
    last_status: c_int,
    _conversation: Box<PamConv>,
    conversation_data: Box<ConversationData>,
}

impl PamHandle {
    pub fn start(
        service: &str,
        user: &str,
        password: Zeroizing<Vec<u8>>,
    ) -> Result<Self, AuthError> {
        let service = cstring("service", service)?;
        let user = cstring("user", user)?;
        let mut conversation_data = Box::new(ConversationData {
            password: Some(password),
        });
        let data_ptr = conversation_data.as_mut() as *mut ConversationData;
        let conversation = Box::new(PamConv::new(data_ptr.cast()));
        let mut raw = ptr::null_mut();

        // SAFETY: service/user are valid for the duration of pam_start. The conversation
        // lives on the heap and is stored inside the handle for the entire PAM session.
        let status = unsafe {
            pam_start(
                service.as_ptr(),
                user.as_ptr(),
                conversation.as_ref(),
                &mut raw,
            )
        };
        if status != PAM_SUCCESS {
            return Err(AuthError::Backend("pam_start failed".to_string()));
        }
        if raw.is_null() {
            return Err(AuthError::Backend(
                "pam_start returned a null handle".to_string(),
            ));
        }
        // Linux-PAM sleeps ~2 seconds inside pam_authenticate on a wrong
        // password (the pam_unix fail delay) unless the application installs
        // its own delay handler. rsdm rate-limits attempts itself and shows
        // the failure on the login screen, so the stock delay only freezes
        // the UI; a no-op handler disables it. Best-effort, like in gdm.
        // SAFETY: raw is a live handle; PAM stores the function pointer for
        // the lifetime of the transaction.
        unsafe {
            pam_set_item(raw, PAM_FAIL_DELAY, no_fail_delay as *const c_void);
        }
        Ok(Self {
            raw,
            last_status: status,
            _conversation: conversation,
            conversation_data,
        })
    }

    pub fn authenticate(&mut self) -> Result<(), AuthError> {
        self.run(pam_authenticate, 0, AuthError::InvalidCredentials)
    }

    pub fn clear_password(&mut self) {
        // Keep the callback context alive for close_session and pam_end, while
        // rejecting any later secret prompt instead of retaining the password.
        self.conversation_data.password.take();
    }

    pub fn account_mgmt(&mut self) -> Result<(), AuthError> {
        self.run(pam_acct_mgmt, 0, AuthError::AccountDenied)
    }

    pub fn establish_credentials(&mut self) -> Result<(), AuthError> {
        self.run_backend(pam_setcred, PAM_ESTABLISH_CRED, "pam_setcred establish")
    }

    pub fn delete_credentials(&mut self) -> Result<(), AuthError> {
        self.run_backend(pam_setcred, PAM_DELETE_CRED, "pam_setcred delete")
    }

    /// Tell PAM which VT/seat this login belongs to and what kind of session it
    /// is. `pam_systemd` reads `PAM_TTY` and the `XDG_SEAT`/`XDG_VTNR`/
    /// `XDG_SESSION_TYPE`/`XDG_SESSION_DESKTOP` PAM environment to register the
    /// logind session on the correct seat so the launched Wayland compositor
    /// can take DRM master. Without an explicit type, pam_systemd derives it
    /// from `PAM_TTY` and registers the compositor's session as `Type=tty` with
    /// no desktop - this is where gdm/sddm publish the same values. Must run
    /// before [`PamHandle::open_session`].
    pub fn prepare_session(
        &mut self,
        tty: &str,
        vtnr: Option<u32>,
        seat: &str,
        session_desktop: Option<&str>,
    ) -> Result<(), AuthError> {
        if !tty.is_empty() {
            let tty_c = cstring("tty", tty)?;
            // SAFETY: raw is a live handle; tty_c stays valid for the call and
            // PAM copies the item internally.
            let status = unsafe { pam_set_item(self.raw, PAM_TTY, tty_c.as_ptr().cast()) };
            self.last_status = status;
            if status != PAM_SUCCESS {
                return Err(AuthError::Session(format!(
                    "pam_set_item(PAM_TTY): {}",
                    self.error(status)
                )));
            }
        }
        if !seat.is_empty() {
            self.putenv(&format!("XDG_SEAT={seat}"))?;
        }
        if let Some(vtnr) = vtnr {
            self.putenv(&format!("XDG_VTNR={vtnr}"))?;
        }
        // rsdm launches Wayland sessions only, so the type is a constant fact of
        // this login, not a guess.
        self.putenv("XDG_SESSION_TYPE=wayland")?;
        if let Some(desktop) = session_desktop.filter(|desktop| !desktop.is_empty()) {
            self.putenv(&format!("XDG_SESSION_DESKTOP={desktop}"))?;
        }
        Ok(())
    }

    fn putenv(&mut self, pair: &str) -> Result<(), AuthError> {
        let pair_c = cstring("pam env", pair)?;
        // SAFETY: raw is a live handle; pair_c is a NUL-terminated KEY=VALUE
        // string valid for the call and copied by PAM.
        let status = unsafe { pam_putenv(self.raw, pair_c.as_ptr()) };
        self.last_status = status;
        if status != PAM_SUCCESS {
            return Err(AuthError::Session(format!(
                "pam_putenv({pair}): {}",
                self.error(status)
            )));
        }
        Ok(())
    }

    pub fn open_session(&mut self) -> Result<(), AuthError> {
        self.run_backend(pam_open_session, 0, "pam_open_session")
    }

    pub fn close_session(&mut self) -> Result<(), AuthError> {
        self.run_backend(pam_close_session, 0, "pam_close_session")
    }

    pub fn environment(&mut self) -> Vec<(String, String)> {
        // SAFETY: pam_getenvlist returns a null-terminated PAM-allocated array.
        // Each entry is a valid C string of the form KEY=VALUE. We copy entries
        // into Rust-owned Strings and free the original list immediately.
        unsafe {
            let list = pam_getenvlist(self.raw);
            if list.is_null() {
                return Vec::new();
            }
            let mut env = Vec::new();
            let mut index = 0;
            loop {
                let item = *list.add(index);
                if item.is_null() {
                    break;
                }
                if let Ok(value) = CStr::from_ptr(item).to_str()
                    && let Some((key, value)) = value.split_once('=')
                {
                    env.push((key.to_string(), value.to_string()));
                }
                libc::free(item.cast());
                index += 1;
            }
            libc::free(list.cast());
            env
        }
    }

    fn run(
        &mut self,
        function: unsafe extern "C" fn(*mut PamHandleRaw, c_int) -> c_int,
        flags: c_int,
        mapped_error: AuthError,
    ) -> Result<(), AuthError> {
        // SAFETY: self.raw is a live PAM handle and flags are PAM-defined.
        let status = unsafe { function(self.raw, flags) };
        self.last_status = status;
        if status == PAM_SUCCESS {
            Ok(())
        } else {
            Err(mapped_error)
        }
    }

    fn run_backend(
        &mut self,
        function: unsafe extern "C" fn(*mut PamHandleRaw, c_int) -> c_int,
        flags: c_int,
        operation: &str,
    ) -> Result<(), AuthError> {
        // SAFETY: self.raw is a live PAM handle and flags are PAM-defined.
        let status = unsafe { function(self.raw, flags) };
        self.last_status = status;
        if status == PAM_SUCCESS {
            Ok(())
        } else {
            Err(AuthError::Session(format!(
                "{operation}: {}",
                self.error(status)
            )))
        }
    }

    fn error(&self, status: c_int) -> String {
        // SAFETY: pam_strerror returns a static string for a live PAM handle.
        let message = unsafe { pam_strerror(self.raw, status) };
        if message.is_null() {
            return format!("PAM status {status}");
        }
        // SAFETY: PAM guarantees a NUL-terminated error string.
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for PamHandle {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: raw is owned by this handle and pam_end is the required finalizer.
            unsafe {
                pam_end(self.raw, self.last_status);
            }
            self.raw = ptr::null_mut();
        }
    }
}

fn cstring(label: &str, value: &str) -> Result<CString, AuthError> {
    CString::new(value).map_err(|_| AuthError::Backend(format!("{label} contains NUL byte")))
}

/// The application-provided PAM fail-delay handler: doing nothing here replaces
/// the library's blocking sleep on failed authentication.
extern "C" fn no_fail_delay(_retval: c_int, _usec_delay: libc::c_uint, _appdata: *mut c_void) {}

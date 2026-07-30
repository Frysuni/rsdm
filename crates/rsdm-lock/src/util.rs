//! Small environment lookups shared by the locker. The banner/clock/hostname
//! helpers moved to `rsdm_ui::banner`; only the seated-user lookup is locker
//! specific.

use std::{ffi::CStr, mem::MaybeUninit, ptr};

/// Resolve the process owner's login name from the effective uid.
///
/// Environment variables such as `USER` are deliberately ignored: they are
/// caller-controlled and must never select the PAM identity used by a locker.
pub fn current_username() -> Option<String> {
    // SAFETY: geteuid never fails and touches no memory.
    let uid = unsafe { libc::geteuid() };
    username_from_passwd(uid)
}

fn username_from_passwd(uid: libc::uid_t) -> Option<String> {
    let mut pwd = MaybeUninit::<libc::passwd>::uninit();
    let mut result = ptr::null_mut();
    let mut buffer = vec![0_u8; passwd_buffer_size()];

    loop {
        // SAFETY: getpwuid_r writes into pwd/buffer (both valid for the call)
        // and sets result; pwd is read only after a non-null result.
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                pwd.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len().saturating_mul(2).min(1024 * 1024), 0);
            continue;
        }
        if status != 0 || result.is_null() {
            return None;
        }
        break;
    }
    // SAFETY: result is non-null, so pwd is initialized and pw_name is a valid
    // NUL-terminated string while buffer lives.
    let name = unsafe { CStr::from_ptr(pwd.assume_init().pw_name) };
    Some(name.to_string_lossy().into_owned())
}

fn passwd_buffer_size() -> usize {
    // SAFETY: sysconf is thread-safe with no pointer invariants.
    let value = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    if value > 0 {
        (value as usize).clamp(1024, 1024 * 1024)
    } else {
        16 * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_effective_uid() {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        assert_eq!(current_username(), username_from_passwd(uid));
        assert!(current_username().is_some());
    }
}

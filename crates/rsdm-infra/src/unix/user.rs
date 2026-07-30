use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    ptr,
};

use rsdm_core::ports::{ResolvedUser, UserResolveError, UserResolver};

use super::group::user_in_any_group;

const MAX_NSS_BUFFER: usize = 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct UnixUserResolver {
    pub deny_root: bool,
    pub allowed_groups: Vec<String>,
}

impl UnixUserResolver {
    pub fn with_allowed_groups(deny_root: bool, allowed_groups: Vec<String>) -> Self {
        Self {
            deny_root,
            allowed_groups,
        }
    }
}

impl UserResolver for UnixUserResolver {
    fn resolve_user(&self, username: &str) -> Result<ResolvedUser, UserResolveError> {
        let user = lookup_user(username)?;
        if self.deny_root && user.uid == 0 {
            return Err(UserResolveError::Denied("root login is denied".to_string()));
        }
        if !self.allowed_groups.is_empty()
            && !user_in_any_group(&user.username, user.gid, &self.allowed_groups)?
        {
            return Err(UserResolveError::Denied(format!(
                "{} is not in an allowed login group",
                user.username
            )));
        }
        Ok(user)
    }
}

fn lookup_user(username: &str) -> Result<ResolvedUser, UserResolveError> {
    let username_c = CString::new(username)
        .map_err(|_| UserResolveError::Backend("username contains NUL".into()))?;
    let mut pwd = MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0_u8; passwd_buffer_size()];

    loop {
        let mut result = ptr::null_mut();
        // SAFETY: getpwnam_r writes into pwd and buffer, both valid for the call.
        let status = unsafe {
            libc::getpwnam_r(
                username_c.as_ptr(),
                pwd.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && grow_nss_buffer(&mut buffer) {
            continue;
        }
        if status != 0 {
            return Err(UserResolveError::Backend(format!(
                "getpwnam_r failed: {status}"
            )));
        }
        if result.is_null() {
            return Err(UserResolveError::NotFound(username.to_string()));
        }
        break;
    }

    // SAFETY: result is non-null, so pwd has been initialized by getpwnam_r.
    let pwd = unsafe { pwd.assume_init() };
    Ok(ResolvedUser {
        username: cstr_to_string(pwd.pw_name)?,
        uid: pwd.pw_uid,
        gid: pwd.pw_gid,
        // An account may have blank passwd fields; login(1) substitutes / and
        // /bin/sh rather than exporting empty HOME/SHELL into the session.
        home: default_if_empty(cstr_to_string(pwd.pw_dir)?, "/"),
        shell: default_if_empty(cstr_to_string(pwd.pw_shell)?, "/bin/sh"),
    })
}

fn default_if_empty(value: String, default: &str) -> String {
    if value.trim().is_empty() {
        default.to_string()
    } else {
        value
    }
}

fn passwd_buffer_size() -> usize {
    // SAFETY: sysconf is thread-safe and has no pointer invariants.
    let value = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    if value > 0 {
        (value as usize).clamp(1024, MAX_NSS_BUFFER)
    } else {
        16 * 1024
    }
}

fn grow_nss_buffer(buffer: &mut Vec<u8>) -> bool {
    if buffer.len() >= MAX_NSS_BUFFER {
        return false;
    }
    buffer.resize(buffer.len().saturating_mul(2).min(MAX_NSS_BUFFER), 0);
    true
}

fn cstr_to_string(value: *const libc::c_char) -> Result<String, UserResolveError> {
    if value.is_null() {
        return Ok(String::new());
    }
    // SAFETY: libc passwd fields are NUL-terminated strings while buffer lives.
    Ok(unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned())
}

#[cfg(test)]
mod tests {
    use super::{MAX_NSS_BUFFER, default_if_empty, grow_nss_buffer};

    #[test]
    fn blank_passwd_fields_fall_back_to_login_defaults() {
        assert_eq!(default_if_empty(String::new(), "/bin/sh"), "/bin/sh");
        assert_eq!(default_if_empty("  ".to_string(), "/"), "/");
        assert_eq!(
            default_if_empty("/home/alice".to_string(), "/"),
            "/home/alice"
        );
    }

    #[test]
    fn nss_buffer_growth_is_bounded() {
        let mut buffer = vec![0; 1024];
        assert!(grow_nss_buffer(&mut buffer));
        assert_eq!(buffer.len(), 2048);

        buffer.resize(MAX_NSS_BUFFER, 0);
        assert!(!grow_nss_buffer(&mut buffer));
    }
}

use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    ptr,
};

const MAX_NSS_BUFFER: usize = 1024 * 1024;
const MAX_USER_GROUPS: usize = 1024 * 1024;

#[derive(Debug)]
pub enum NssError {
    InvalidName(&'static str),
    System { operation: &'static str, code: i32 },
    TooManyGroups,
}

impl std::fmt::Display for NssError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(kind) => write!(formatter, "{kind} contains NUL"),
            Self::System { operation, code } => write!(formatter, "{operation} failed: {code}"),
            Self::TooManyGroups => formatter.write_str("user belongs to too many groups"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserRecord {
    pub username: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
}

enum PasswdLookup<'a> {
    Name(&'a CStr),
    Uid(libc::uid_t),
}

pub fn user_by_name(name: &str) -> Result<Option<UserRecord>, NssError> {
    let name = CString::new(name).map_err(|_| NssError::InvalidName("username"))?;
    lookup_passwd(PasswdLookup::Name(&name))
}

pub fn user_name_by_uid(uid: libc::uid_t) -> Result<Option<String>, NssError> {
    Ok(lookup_passwd(PasswdLookup::Uid(uid))?.map(|user| user.username))
}

pub fn uid_by_name(name: &str) -> Result<Option<u32>, NssError> {
    Ok(user_by_name(name)?.map(|user| user.uid))
}

pub fn group_gid(name: &str) -> Result<Option<u32>, NssError> {
    let name = CString::new(name).map_err(|_| NssError::InvalidName("group"))?;
    let mut group = MaybeUninit::<libc::group>::uninit();
    let mut buffer = vec![0_u8; group_buffer_size()];

    loop {
        let mut result = ptr::null_mut();
        // SAFETY: getgrnam_r writes into group and buffer, both valid for the call.
        let status = unsafe {
            libc::getgrnam_r(
                name.as_ptr(),
                group.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && grow_buffer(&mut buffer) {
            continue;
        }
        if status != 0 {
            return Err(NssError::System {
                operation: "getgrnam_r",
                code: status,
            });
        }
        if result.is_null() {
            return Ok(None);
        }

        // SAFETY: result is non-null, so group was initialized by getgrnam_r.
        return Ok(Some(unsafe { group.assume_init() }.gr_gid));
    }
}

pub fn groups_for_user(username: &str, primary_gid: u32) -> Result<Vec<u32>, NssError> {
    let username = CString::new(username).map_err(|_| NssError::InvalidName("username"))?;
    let mut capacity = 16_usize;

    loop {
        let mut groups = vec![0 as libc::gid_t; capacity];
        let mut count = groups.len() as libc::c_int;
        // SAFETY: getgrouplist writes at most count gids into groups.
        let status = unsafe {
            libc::getgrouplist(
                username.as_ptr(),
                primary_gid,
                groups.as_mut_ptr(),
                &mut count,
            )
        };
        if status >= 0 {
            groups.truncate(count as usize);
            return Ok(groups);
        }

        let needed = count.max(0) as usize;
        let next = needed.max(capacity.saturating_mul(2));
        if next > MAX_USER_GROUPS {
            return Err(NssError::TooManyGroups);
        }
        capacity = next;
    }
}

fn lookup_passwd(key: PasswdLookup<'_>) -> Result<Option<UserRecord>, NssError> {
    let mut pwd = MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0_u8; passwd_buffer_size()];

    loop {
        let mut result = ptr::null_mut();
        // SAFETY: getpw*_r writes into pwd and buffer, both valid for the call.
        let status = unsafe {
            match key {
                PasswdLookup::Name(name) => libc::getpwnam_r(
                    name.as_ptr(),
                    pwd.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                ),
                PasswdLookup::Uid(uid) => libc::getpwuid_r(
                    uid,
                    pwd.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                ),
            }
        };
        if status == libc::ERANGE && grow_buffer(&mut buffer) {
            continue;
        }
        if status != 0 {
            return Err(NssError::System {
                operation: "getpw*_r",
                code: status,
            });
        }
        if result.is_null() {
            return Ok(None);
        }

        // SAFETY: result is non-null, so pwd was initialized by getpw*_r.
        let pwd = unsafe { pwd.assume_init() };
        return Ok(Some(UserRecord {
            username: cstr_to_string(pwd.pw_name),
            uid: pwd.pw_uid,
            gid: pwd.pw_gid,
            home: cstr_to_string(pwd.pw_dir),
            shell: cstr_to_string(pwd.pw_shell),
        }));
    }
}

fn cstr_to_string(value: *const libc::c_char) -> String {
    if value.is_null() {
        return String::new();
    }
    // SAFETY: libc passwd fields are NUL-terminated strings while buffer lives.
    unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned()
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

fn group_buffer_size() -> usize {
    // SAFETY: sysconf is thread-safe and has no pointer invariants.
    let value = unsafe { libc::sysconf(libc::_SC_GETGR_R_SIZE_MAX) };
    if value > 0 {
        (value as usize).clamp(1024, MAX_NSS_BUFFER)
    } else {
        16 * 1024
    }
}

fn grow_buffer(buffer: &mut Vec<u8>) -> bool {
    if buffer.len() >= MAX_NSS_BUFFER {
        return false;
    }
    buffer.resize(buffer.len().saturating_mul(2).min(MAX_NSS_BUFFER), 0);
    true
}

#[cfg(test)]
mod tests {
    use super::{MAX_NSS_BUFFER, grow_buffer};

    #[test]
    fn nss_buffer_growth_is_bounded() {
        let mut buffer = vec![0; 1024];
        assert!(grow_buffer(&mut buffer));
        assert_eq!(buffer.len(), 2048);

        buffer.resize(MAX_NSS_BUFFER, 0);
        assert!(!grow_buffer(&mut buffer));
    }
}

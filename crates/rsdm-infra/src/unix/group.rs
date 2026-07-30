use std::{ffi::CString, mem::MaybeUninit, ptr};

use rsdm_core::ports::UserResolveError;

const MAX_NSS_BUFFER: usize = 1024 * 1024;
const MAX_USER_GROUPS: usize = 1024 * 1024;

pub fn user_in_any_group(
    username: &str,
    primary_gid: u32,
    allowed_groups: &[String],
) -> Result<bool, UserResolveError> {
    let allowed_gids = allowed_groups
        .iter()
        .map(|name| lookup_group_gid(name))
        .collect::<Result<Vec<_>, _>>()?;

    if allowed_gids.contains(&primary_gid) {
        return Ok(true);
    }

    let user_gids = user_groups(username, primary_gid)?;
    Ok(user_gids.iter().any(|gid| allowed_gids.contains(gid)))
}

fn lookup_group_gid(name: &str) -> Result<u32, UserResolveError> {
    let name_c =
        CString::new(name).map_err(|_| UserResolveError::Backend("group contains NUL".into()))?;
    let mut group = MaybeUninit::<libc::group>::uninit();
    let mut buffer = vec![0_u8; group_buffer_size()];

    loop {
        let mut result = ptr::null_mut();
        // SAFETY: getgrnam_r writes into group and buffer, both valid for the call.
        let status = unsafe {
            libc::getgrnam_r(
                name_c.as_ptr(),
                group.as_mut_ptr(),
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
                "getgrnam_r failed: {status}"
            )));
        }
        if result.is_null() {
            return Err(UserResolveError::Denied(format!(
                "allowed group does not exist: {name}"
            )));
        }
        break;
    }

    // SAFETY: result is non-null, so group was initialized by getgrnam_r.
    Ok(unsafe { group.assume_init() }.gr_gid)
}

fn user_groups(username: &str, primary_gid: u32) -> Result<Vec<u32>, UserResolveError> {
    let username_c = CString::new(username)
        .map_err(|_| UserResolveError::Backend("username contains NUL".into()))?;
    let mut capacity = 16_usize;

    loop {
        let mut groups = vec![0 as libc::gid_t; capacity];
        let mut count = groups.len() as libc::c_int;
        // SAFETY: getgrouplist writes at most count gids into groups.
        let status = unsafe {
            libc::getgrouplist(
                username_c.as_ptr(),
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
            return Err(UserResolveError::Backend(
                "user belongs to too many groups".to_string(),
            ));
        }
        capacity = next;
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

fn grow_nss_buffer(buffer: &mut Vec<u8>) -> bool {
    if buffer.len() >= MAX_NSS_BUFFER {
        return false;
    }
    buffer.resize(buffer.len().saturating_mul(2).min(MAX_NSS_BUFFER), 0);
    true
}

use std::{ffi::CString, io, process::Command, time::Duration};

use anyhow::{Context, Result};

pub fn run(user: Option<&str>, resolved_uid: Option<u32>) -> Result<()> {
    let current_uid = rsdm_infra::lock_control::current_uid();
    let target_uid = resolve_target_uid(user, resolved_uid, current_uid)?;

    if current_uid != 0 {
        return elevate(target_uid);
    }

    let state =
        rsdm_infra::lock_control::request_emergency_unlock(target_uid, Duration::from_secs(5))?;
    println!(
        "emergency unlock acknowledged for uid {} (lock pid {})",
        state.uid, state.pid
    );
    Ok(())
}

fn resolve_target_uid(
    user: Option<&str>,
    resolved_uid: Option<u32>,
    current_uid: u32,
) -> Result<u32> {
    if let Some(uid) = resolved_uid {
        return Ok(uid);
    }
    if let Some(user) = user {
        return resolve_uid(user);
    }
    if current_uid != 0 {
        return Ok(current_uid);
    }
    if let Some(uid) = sudo_uid() {
        return Ok(uid);
    }

    match rsdm_infra::lock_control::locked_users().as_slice() {
        [uid] => Ok(*uid),
        [] => anyhow::bail!("no active rsdm lock was found"),
        _ => anyhow::bail!("multiple users are locked; specify --user <name>"),
    }
}

fn sudo_uid() -> Option<u32> {
    std::env::var("SUDO_UID")
        .ok()
        .and_then(|uid| uid.parse().ok())
}

fn elevate(uid: u32) -> Result<()> {
    let executable = std::env::current_exe().context("resolving the rsdm executable")?;
    let uid = uid.to_string();
    let status = Command::new("sudo")
        .arg("--")
        .arg(&executable)
        .arg("unlock")
        .arg("--uid")
        .arg(&uid)
        .status();

    match status {
        Ok(status) if status.success() => return Ok(()),
        Ok(status) => anyhow::bail!("sudo did not authorize emergency unlock ({status})"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("starting sudo for emergency unlock"),
    }

    let status = Command::new("pkexec")
        .arg(&executable)
        .arg("unlock")
        .arg("--uid")
        .arg(uid)
        .status()
        .context("neither sudo nor a working pkexec is available for emergency unlock")?;
    if !status.success() {
        anyhow::bail!("pkexec did not authorize emergency unlock ({status})");
    }
    Ok(())
}

fn resolve_uid(user: &str) -> Result<u32> {
    if let Ok(uid) = user.parse() {
        return Ok(uid);
    }

    let name = CString::new(user).context("user name contains a NUL byte")?;
    let initial = passwd_buffer_size();
    let mut buffer = vec![0_u8; initial];
    let mut passwd = std::mem::MaybeUninit::<libc::passwd>::uninit();

    loop {
        let mut result = std::ptr::null_mut();
        // SAFETY: all pointers refer to live writable storage for this call.
        let code = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                passwd.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if code == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len().saturating_mul(2).min(1024 * 1024), 0);
            continue;
        }
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code)).context("looking up unlock user");
        }
        if result.is_null() {
            anyhow::bail!("unknown user {user:?}");
        }
        // SAFETY: a non-null result means getpwnam_r initialized passwd.
        return Ok(unsafe { passwd.assume_init() }.pw_uid);
    }
}

fn passwd_buffer_size() -> usize {
    // SAFETY: sysconf has no pointer invariants and is thread-safe.
    let recommended = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    if recommended > 0 {
        (recommended as usize).clamp(1024, 1024 * 1024)
    } else {
        16 * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_uid_does_not_require_nss() {
        assert_eq!(resolve_uid("1234").expect("numeric uid"), 1234);
    }
}

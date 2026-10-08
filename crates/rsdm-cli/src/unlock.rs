use std::{io, process::Command, time::Duration};

use anyhow::{Context, Result};

pub fn run(user: Option<&str>, resolved_uid: Option<u32>) -> Result<()> {
    let current_uid = rsdm_infra::lock_control::current_uid();
    let target_uid = resolve_target_uid(user, resolved_uid, current_uid)?;

    if current_uid != 0 {
        return elevate(target_uid);
    }

    let state =
        rsdm_infra::lock_control::request_emergency_unlock(target_uid, Duration::from_secs(5))?;
    crate::output::notice("EMERGENCY UNLOCK ACKNOWLEDGED", format!(
        "emergency unlock acknowledged for uid {} (lock pid {})\n",
        state.uid, state.pid
    ), crate::output::SUCCESS).stdout()?;
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

    let uid = rsdm_infra::unix::uid_for_username(user)
        .map_err(|error| anyhow::anyhow!("looking up unlock user: {error}"))?;
    uid.ok_or_else(|| anyhow::anyhow!("unknown user {user:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_uid_does_not_require_nss() {
        assert_eq!(resolve_uid("1234").expect("numeric uid"), 1234);
    }
}

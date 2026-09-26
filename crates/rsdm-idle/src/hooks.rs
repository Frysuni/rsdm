//! Hook execution follows confirmed activation and successful locker exit.

use std::{
    io,
    process::{Command, ExitStatus},
};

pub(super) fn finish_lock_cycle(
    result: io::Result<ExitStatus>,
    lock_confirmed: bool,
    on_unlock: &[String],
) {
    match result {
        Ok(status) if status.success() && lock_confirmed => {
            tracing::info!("idle locker exited after unlock");
            run_hooks("on_unlock", on_unlock);
        }
        Ok(status) if status.success() => {
            tracing::warn!("skipping on_unlock hooks because lock activation was never confirmed");
        }
        Ok(status) => tracing::error!(%status, "idle locker failed; skipping on_unlock hooks"),
        Err(error) => {
            tracing::error!(%error, "failed waiting for idle locker; skipping on_unlock hooks")
        }
    }
}

pub(super) fn run_hooks(phase: &str, hooks: &[String]) {
    for hook in hooks {
        tracing::info!(%phase, command = %hook, "running idle hook");
        match Command::new("sh").arg("-c").arg(hook).status() {
            Ok(status) if status.success() => {}
            Ok(status) => tracing::warn!(%phase, %status, command = %hook, "idle hook failed"),
            Err(error) => {
                tracing::warn!(%phase, %error, command = %hook, "could not start idle hook")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn unlock_hooks_require_confirmation_and_successful_exit() {
        let marker = std::env::temp_dir().join(format!("rsdm-unlock-hook-{}", std::process::id()));
        let hooks = [format!(
            "printf unlocked > '{}'",
            marker.display().to_string().replace('\'', "'\\''")
        )];
        for result in [
            Ok(ExitStatus::from_raw(1 << 8)),
            Ok(ExitStatus::from_raw(9)),
            Err(io::Error::other("wait failed")),
        ] {
            finish_lock_cycle(result, true, &hooks);
            assert!(!marker.exists());
        }
        finish_lock_cycle(Ok(ExitStatus::from_raw(0)), false, &hooks);
        assert!(!marker.exists());
        finish_lock_cycle(Ok(ExitStatus::from_raw(0)), true, &hooks);
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "unlocked");
        std::fs::remove_file(marker).unwrap();
    }
}

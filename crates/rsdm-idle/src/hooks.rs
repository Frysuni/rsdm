//! Hook execution follows confirmed activation and successful locker exit.

use std::{
    io,
    process::{Command, ExitStatus},
    time::{Duration, Instant},
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
    run_hooks_until(phase, hooks, Instant::now() + Duration::from_secs(30));
}

fn run_hooks_until(phase: &str, hooks: &[String], deadline: Instant) {
    for hook in hooks {
        if Instant::now() >= deadline {
            tracing::warn!(%phase, "idle hook budget exhausted; skipping remaining hooks");
            break;
        }
        tracing::info!(%phase, command = %hook, "running idle hook");
        let mut command = Command::new("sh");
        command.arg("-c").arg(hook);
        match rsdm_infra::unix::run_command_until(&mut command, deadline) {
            Ok(status) if status.success() => {}
            Ok(status) => tracing::warn!(%phase, %status, command = %hook, "idle hook failed"),
            Err(error) => {
                tracing::warn!(%phase, %error, command = %hook, "idle hook did not complete");
                if error.kind() == io::ErrorKind::TimedOut { break; }
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

    #[test]
    fn a_timed_out_hook_skips_the_remaining_phase_and_does_not_block_a_later_cycle() {
        let marker = std::env::temp_dir().join(format!("rsdm-hook-timeout-{}", std::process::id()));
        let write = format!("printf recovered > '{}'", marker.display().to_string().replace('\'', "'\\''"));
        let hooks = ["kill -STOP $$; exit 0".into(), write.clone()];
        let before = Instant::now();
        run_hooks_until("on_lock", &hooks, before + Duration::from_millis(100));
        let elapsed = before.elapsed();
        let skipped = !marker.exists();
        run_hooks("on_unlock", &[write]);
        let recovered = std::fs::read_to_string(&marker).unwrap();
        std::fs::remove_file(marker).unwrap();
        assert!(elapsed < Duration::from_secs(1));
        assert!(skipped);
        assert_eq!(recovered, "recovered");
    }

    #[test]
    fn ordinary_hook_failures_still_allow_later_hooks_inside_the_budget() {
        let marker = std::env::temp_dir().join(format!("rsdm-hook-failure-{}", std::process::id()));
        let hooks = ["exit 7".into(), format!("printf next > '{}'", marker.display().to_string().replace('\'', "'\\''"))];
        run_hooks_until("on_lock", &hooks, Instant::now() + Duration::from_secs(1));
        let result = std::fs::read_to_string(&marker).unwrap();
        std::fs::remove_file(marker).unwrap();
        assert_eq!(result, "next");
    }
}

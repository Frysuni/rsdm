use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    sync::{Mutex, MutexGuard, Once, OnceLock},
};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute,
    terminal::{
        Clear, ClearType, DisableLineWrap, EnableLineWrap, EnterAlternateScreen,
        LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    },
};

static PANIC_HOOK: Once = Once::new();
static ACTIVE_TTY: OnceLock<Mutex<Option<String>>> = OnceLock::new();

pub(super) struct TerminalGuard {
    tty_path: String,
    _console_log: ConsoleLogGuard,
}

impl TerminalGuard {
    pub(super) fn enter(tty_path: &str, mut output: File) -> Result<Self, io::Error> {
        let console_log = ConsoleLogGuard::quiet();
        enable_raw_mode()?;
        if let Err(error) = execute!(
            output,
            EnterAlternateScreen,
            Clear(ClearType::All),
            MoveTo(0, 0),
            Hide,
            DisableLineWrap
        ) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        output.flush()?;
        set_active_tty(Some(tty_path.to_string()));
        Ok(Self {
            tty_path: tty_path.to_string(),
            _console_log: console_log,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Ok(mut output) = open_tty(&self.tty_path) {
            // Clear AFTER leaving the alternate screen: leaving restores the
            // primary screen, which still holds whatever scrolled there before
            // the greeter started (boot noise, a previous run's leftovers).
            // Without the wipe that stale text is what stays on the VT through
            // a session handoff or a shutdown and makes the console look broken.
            let _ = execute!(
                output,
                Show,
                EnableLineWrap,
                LeaveAlternateScreen,
                Clear(ClearType::All),
                MoveTo(0, 0)
            );
            let _ = output.flush();
        }
        let _ = disable_raw_mode();
        set_active_tty(None);
    }
}

pub(super) fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            if let Some(path) = active_tty()
                && let Ok(mut output) = open_tty(&path)
            {
                let _ = execute!(output, Show, EnableLineWrap, LeaveAlternateScreen);
                let _ = output.flush();
            }
            let _ = disable_raw_mode();
            previous(panic_info);
        }));
    });
}

pub(super) fn open_tty(path: &str) -> Result<File, io::Error> {
    OpenOptions::new().read(true).write(true).open(path)
}

fn set_active_tty(path: Option<String>) {
    *active_tty_slot() = path;
}

fn active_tty() -> Option<String> {
    if ACTIVE_TTY.get().is_some() {
        active_tty_slot().clone()
    } else {
        None
    }
}

fn active_tty_slot() -> MutexGuard<'static, Option<String>> {
    ACTIVE_TTY
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct ConsoleLogGuard {
    previous: Option<String>,
    restore_systemd_status: bool,
}

impl ConsoleLogGuard {
    fn quiet() -> Self {
        let path = "/proc/sys/kernel/printk";
        let previous = std::fs::read_to_string(path).ok();
        if let Some(current) = previous.as_deref()
            && let Some(quiet) = quiet_printk_value(current)
        {
            let _ = std::fs::write(path, quiet);
        }
        // printk quieting only covers *kernel* messages. systemd (PID 1) prints
        // unit status ("[ OK ] Started ...") straight to /dev/console, which is
        // usually the greeter's VT, so ask it to stop painting status while we
        // own the screen. Re-enabled on drop. Needs privilege (the greeter runs
        // as root before it drops); a failure is harmless.
        let restore_systemd_status = set_systemd_console_status(false);
        Self {
            previous,
            restore_systemd_status,
        }
    }
}

impl Drop for ConsoleLogGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            let _ = std::fs::write("/proc/sys/kernel/printk", previous);
        }
        if self.restore_systemd_status {
            set_systemd_console_status(true);
        }
    }
}

/// Toggle systemd's console status output via the documented PID 1 real-time
/// signals (`SIGRTMIN+20` enables, `SIGRTMIN+21` disables). Returns whether the
/// signal was delivered, so the caller only restores what it changed.
fn set_systemd_console_status(enabled: bool) -> bool {
    // SIGRTMIN is libc-defined, not a fixed constant.
    let signal = libc::SIGRTMIN() + if enabled { 20 } else { 21 };
    // SAFETY: kill(2) with a valid signal targeting PID 1 has no memory effects.
    unsafe { libc::kill(1, signal) == 0 }
}

fn quiet_printk_value(current: &str) -> Option<String> {
    let mut parts = current.split_whitespace().collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    parts[0] = "1";
    Some(format!("{}\n", parts.join("\t")))
}

#[cfg(test)]
mod tests {
    use super::quiet_printk_value;

    #[test]
    fn quiet_printk_preserves_the_other_loglevels() {
        assert_eq!(
            quiet_printk_value("7 4 1 7\n").as_deref(),
            Some("1\t4\t1\t7\n")
        );
    }
}

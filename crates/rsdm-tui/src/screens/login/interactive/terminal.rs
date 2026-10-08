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

use super::console_log::ConsoleLogGuard;

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

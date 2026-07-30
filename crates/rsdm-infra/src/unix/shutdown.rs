//! Cooperative service-stop flag for the greeter.
//!
//! Without a handler, SIGTERM kills the greeter mid-frame: the VT is left in
//! raw mode on the alternate screen, and whatever the greeter last drew stays
//! on the console through shutdown. With this handler the signal only raises a
//! flag; the greeter's loops notice it, restore and clear the terminal, and
//! exit cleanly. The handler is installed WITHOUT `SA_RESTART` on purpose so
//! blocking calls (the session report pipe, the input poll) return `EINTR`
//! instead of resuming, which is what lets those loops see the flag at all.

use std::sync::atomic::{AtomicBool, Ordering};

static TERMINATE: AtomicBool = AtomicBool::new(false);
static EMERGENCY_UNLOCK: AtomicBool = AtomicBool::new(false);

/// The shared stop flag, for callers that poll it themselves (the login UI).
pub fn terminate_flag() -> &'static AtomicBool {
    &TERMINATE
}

/// Whether a service stop has been requested.
pub fn terminate_requested() -> bool {
    TERMINATE.load(Ordering::SeqCst)
}

/// Install the SIGTERM handler. Call once, early in the greeter.
pub fn install_terminate_handler() {
    // SAFETY: the handler only stores into an atomic (async-signal-safe);
    // sigaction is called with a zeroed, then fully initialized struct.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_terminate as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_flags = 0;
        libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
    }
}

/// Whether a privileged `rsdm unlock` request has reached the locker.
pub fn emergency_unlock_requested() -> bool {
    EMERGENCY_UNLOCK.load(Ordering::SeqCst)
}

/// Install the locker's SIGUSR1 handler. The signal only raises an atomic flag;
/// the Wayland event loop performs the actual protocol request.
pub fn install_emergency_unlock_handler() {
    EMERGENCY_UNLOCK.store(false, Ordering::SeqCst);
    // SAFETY: the handler only stores into an atomic and sigaction is fully
    // initialized before installation.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_emergency_unlock as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_flags = libc::SA_SIGINFO;
        libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut());
    }
}

extern "C" fn on_terminate(_signal: libc::c_int) {
    TERMINATE.store(true, Ordering::SeqCst);
}

extern "C" fn on_emergency_unlock(
    _signal: libc::c_int,
    info: *mut libc::siginfo_t,
    _context: *mut libc::c_void,
) {
    // SAFETY: SA_SIGINFO guarantees a valid siginfo pointer for this handler.
    // The kernel, not the sender, supplies si_uid. Same-UID SIGUSR1 therefore
    // cannot turn the emergency path into an authentication bypass.
    if !info.is_null() && unsafe { (*info).si_uid() } == 0 {
        EMERGENCY_UNLOCK.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigusr1_accepts_only_a_root_sender() {
        install_emergency_unlock_handler();
        // SAFETY: getpid has no preconditions and this module installed SIGUSR1.
        let result = unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) };
        assert_eq!(result, 0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !emergency_unlock_requested() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(
            emergency_unlock_requested(),
            // SAFETY: geteuid has no preconditions.
            unsafe { libc::geteuid() } == 0
        );
    }
}

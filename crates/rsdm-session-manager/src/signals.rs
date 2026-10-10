use std::sync::atomic::{AtomicBool, Ordering};

static TERMINATE: AtomicBool = AtomicBool::new(false);

pub(super) fn install() {
    TERMINATE.store(false, Ordering::SeqCst);
    // SAFETY: the handler performs only an atomic store.
    unsafe {
        let handler = on_terminate as *const () as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }
}

pub(super) fn requested() -> bool { TERMINATE.swap(false, Ordering::SeqCst) }

extern "C" fn on_terminate(_signal: libc::c_int) { TERMINATE.store(true, Ordering::SeqCst); }

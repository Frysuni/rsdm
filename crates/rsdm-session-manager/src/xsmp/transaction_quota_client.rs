//! Checkpoint requests expose replies while the server's request pool is held.

use super::*;

unsafe extern "C" {
    fn SmcRequestSaveYourself(connection: SmcConn, kind: c_int, shutdown: c_int,
        style: c_int, fast: c_int, global: c_int);
}

pub(super) fn request(connection: SmcConn) {
    let _ = std::io::stdin().read_exact(&mut [0_u8]);
    unsafe { SmcRequestSaveYourself(connection, 1, 0, 0, 0, 0); }
    println!("xsmp-client-local-requested");
    let _ = std::io::stdout().flush();
}

pub(super) fn save(connection: SmcConn, state: &mut State) {
    state.local_saves += 1;
    println!("xsmp-client-local-saving");
    let _ = std::io::stdout().flush();
    let _ = std::io::stdin().read_exact(&mut [0_u8]);
    unsafe { SmcSaveYourselfDone(connection, 1); }
    println!("xsmp-client-local-done");
    let _ = std::io::stdout().flush();
}

pub(super) fn complete() {
    println!("xsmp-client-local-complete");
    let _ = std::io::stdout().flush();
    DISCONNECTED.store(true, Ordering::SeqCst);
}

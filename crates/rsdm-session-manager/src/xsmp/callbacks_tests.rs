//! An in-flight ICE dispatch must stop before issuing another libSM request.

use super::*;

#[test]
fn closed_admission_rejects_callbacks_before_touching_libsm_connections() {
    let mut peer = Peer::disabled_for_test();
    let data = (&mut *peer as *mut Peer).cast();
    let admission = Admission { peer: Cell::new(&mut *peer) };
    let mut mask = 42;
    let mut callbacks = std::mem::MaybeUninit::uninit();
    // This deliberately invalid connection must never reach libSM. Dispatch
    // may have begun before disable, but none of its callbacks can start work.
    let sms = 1_usize as SmsConn;
    unsafe {
        assert_eq!(new_client(sms, (&admission as *const Admission).cast_mut().cast(),
            &mut mask, callbacks.as_mut_ptr(), ptr::null_mut()), 0);
        assert_eq!(register(sms, data, ptr::null_mut()), 0);
        phase2(sms, data);
        get_properties(sms, data);
    }
    assert_eq!(mask, 42);
    assert_eq!(peer.phase, Phase::InitialSave);
    assert!(peer.sms.is_null());
}

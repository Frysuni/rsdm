//! Keep only Unix-domain ICE listeners and close every network listener.

use std::{ffi::CStr, ptr};

use super::ffi;
use crate::session_manager::SessionError;

pub(super) struct Listeners {
    objects: *mut ffi::ListenObj,
    count: usize,
    pub networks: Vec<String>,
}

impl Listeners {
    pub fn open() -> Result<Self, SessionError> {
        let mut count = 0;
        let mut objects = ptr::null_mut();
        let mut error: [libc::c_char; 256] = [0; 256];
        // SAFETY: libICE writes its owned listener array and a bounded error.
        if unsafe { ffi::IceListenForConnections(&mut count, &mut objects, 256, error.as_mut_ptr()) } == 0 {
            return Err(SessionError::State("could not create ICE listeners".into()));
        }
        let mut listeners = Self { objects, count: count as usize, networks: Vec::new() };
        listeners.retain_local()?;
        for index in 0..listeners.count {
            let object = listeners.object(index);
            // SAFETY: object remains owned by listeners; the returned string is
            // malloc-owned and is released after copying it.
            let network = unsafe { ffi::IceGetListenConnectionString(object) };
            if network.is_null() { return Err(SessionError::State("ICE listener has no network ID".into())); }
            let name = unsafe { CStr::from_ptr(network) }.to_string_lossy().into_owned();
            unsafe { libc::free(network.cast()); ffi::IceSetHostBasedAuthProc(object, None); }
            listeners.networks.push(name);
        }
        Ok(listeners)
    }

    fn retain_local(&mut self) -> Result<(), SessionError> {
        let mut local = 0;
        for index in 0..self.count {
            let fd = self.fd(index);
            if unix_socket(fd)? {
                // SAFETY: both indices are in the libICE-owned pointer array.
                unsafe { ptr::swap(self.objects.add(local), self.objects.add(index)); }
                local += 1;
            }
        }
        if local == 0 { return Err(SessionError::State("ICE has no Unix-domain listener".into())); }
        // IceFreeListenObjs also frees the pointer array. Transfer local objects
        // to a separately malloc-owned array before closing all remote objects.
        let retained = unsafe { libc::malloc(local * size_of::<ffi::ListenObj>()) }.cast::<ffi::ListenObj>();
        if retained.is_null() { return Err(std::io::Error::from_raw_os_error(libc::ENOMEM).into()); }
        unsafe {
            ptr::copy_nonoverlapping(self.objects, retained, local);
            ptr::copy(self.objects.add(local), self.objects, self.count - local);
            ffi::IceFreeListenObjs((self.count - local) as i32, self.objects);
        }
        self.objects = retained;
        self.count = local;
        Ok(())
    }

    pub fn count(&self) -> usize { self.count }

    pub fn object(&self, index: usize) -> ffi::ListenObj {
        assert!(index < self.count);
        // SAFETY: the checked index addresses a live listener we own.
        unsafe { *self.objects.add(index) }
    }

    pub fn fd(&self, index: usize) -> i32 {
        // SAFETY: object belongs to this listener array.
        unsafe { ffi::IceGetListenConnectionNumber(self.object(index)) }
    }
}

impl Drop for Listeners {
    fn drop(&mut self) {
        // SAFETY: these objects and their malloc-owned array have one owner.
        unsafe { ffi::IceFreeListenObjs(self.count as i32, self.objects); }
    }
}

fn unix_socket(fd: i32) -> Result<bool, SessionError> {
    // SAFETY: sockaddr_storage contains integer fields and padding.
    let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    // SAFETY: address and length are writable and cover the complete structure.
    if unsafe { libc::getsockname(fd, (&mut address as *mut libc::sockaddr_storage).cast(), &mut length) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(address.ss_family as i32 == libc::AF_UNIX)
}

//! Prepare private ICE credentials without touching libICE globals.

use std::{ffi::{CStr, CString}, fs::{self, OpenOptions}, io::{Read, Write},
    os::unix::fs::OpenOptionsExt, path::PathBuf};

use zeroize::Zeroizing;

use crate::SessionError;

pub(super) struct Cookie {
    pub(super) protocol: &'static CStr,
    pub(super) network: CString,
    pub(super) data: Zeroizing<[u8; 16]>,
}

pub(super) struct PreparedAuthority {
    pub(super) path: Option<PathBuf>,
    pub(super) cookies: Vec<Cookie>,
    writer_thread: std::thread::ThreadId,
}

pub(super) fn prepare(directory: PathBuf, networks: Vec<String>) -> Result<PreparedAuthority, SessionError> {
    let path = directory.join("ICEauthority");
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&path)?;
    let mut prepared = PreparedAuthority {
        path: Some(path), cookies: Vec::new(), writer_thread: std::thread::current().id(),
    };
    for network in networks {
        let network = CString::new(network).map_err(|_| SessionError::State("invalid ICE network ID".into()))?;
        for protocol in [c"ICE", c"XSMP"] {
            let mut data = Zeroizing::new([0_u8; 16]);
            fs::File::open("/dev/urandom")?.read_exact(data.as_mut())?;
            for value in [protocol.to_bytes(), &[], network.as_bytes(), c"MIT-MAGIC-COOKIE-1".to_bytes(), data.as_slice()] {
                write_field(&mut file, value)?;
            }
            prepared.cookies.push(Cookie { protocol, network: network.clone(), data });
        }
    }
    file.flush()?;
    Ok(prepared)
}

impl Drop for PreparedAuthority {
    fn drop(&mut self) {
        let Some(path) = self.path.take() else { return; };
        // An unfinished job drops its unclaimed output on the writer thread;
        // a completed output can instead drop where its JoinHandle is dropped.
        // Do not enqueue another job while the blocking pool drains its writer.
        if std::thread::current().id() == self.writer_thread {
            let _ = fs::remove_file(path);
        } else {
            tokio::task::spawn_blocking(move || { let _ = fs::remove_file(path); });
        }
    }
}

fn write_field(file: &mut impl Write, value: &[u8]) -> Result<(), SessionError> {
    let length = u16::try_from(value.len()).map_err(|_| SessionError::State("ICE authority field is too long".into()))?;
    file.write_all(&length.to_be_bytes())?;
    Ok(file.write_all(value)?)
}

#[cfg(test)]
#[path = "authority_file_tests.rs"]
mod tests;

//! Private ICE authority and the server's independent in-memory cookies.

use std::{ffi::CString, fs::{self, OpenOptions}, io::{Read, Write}, os::unix::fs::OpenOptionsExt, path::{Path, PathBuf}};

use zeroize::Zeroizing;

use super::ffi;
use crate::session_manager::SessionError;

pub(super) struct Authority {
    pub path: PathBuf,
}

impl Authority {
    pub fn create(directory: &Path, networks: &[String]) -> Result<Self, SessionError> {
        let path = directory.join("ICEauthority");
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&path)?;
        let authority = Self { path };
        let auth_name = c"MIT-MAGIC-COOKIE-1";
        for network in networks {
            let network = CString::new(network.as_str()).map_err(|_| SessionError::State("invalid ICE network ID".into()))?;
            for protocol in [c"ICE", c"XSMP"] {
                let mut cookie = Zeroizing::new([0_u8; 16]);
                std::fs::File::open("/dev/urandom")?.read_exact(cookie.as_mut())?;
                for value in [protocol.to_bytes(), &[], network.as_bytes(), auth_name.to_bytes(), cookie.as_slice()] {
                    write_field(&mut file, value)?;
                }
                let mut entry = ffi::AuthData {
                    protocol: protocol.as_ptr().cast_mut(), network: network.as_ptr().cast_mut(),
                    name: auth_name.as_ptr().cast_mut(), length: 16, data: cookie.as_mut_ptr().cast(),
                };
                // SAFETY: IceSetPaAuthData copies all strings and cookie bytes.
                unsafe { ffi::IceSetPaAuthData(1, &mut entry); }
            }
        }
        file.flush()?;
        Ok(authority)
    }
}

impl Drop for Authority {
    fn drop(&mut self) { let _ = fs::remove_file(&self.path); }
}

fn write_field(file: &mut impl Write, value: &[u8]) -> Result<(), SessionError> {
    let length = u16::try_from(value.len()).map_err(|_| SessionError::State("ICE authority field is too long".into()))?;
    file.write_all(&length.to_be_bytes())?;
    Ok(file.write_all(value)?)
}

//! Private ICE authority and the server's independent in-memory cookies.

use std::path::{Path, PathBuf};

use super::ffi;
use crate::SessionError;

#[path = "authority_file.rs"]
mod file;

pub(super) struct Authority {
    pub path: PathBuf,
}

impl Authority {
    pub async fn create(directory: &Path, networks: &[String]) -> Result<Self, SessionError> {
        let directory = directory.to_path_buf();
        let networks = networks.to_vec();
        let mut prepared = crate::runtime::blocking(move || file::prepare(directory, networks)).await?;

        let auth_name = c"MIT-MAGIC-COOKIE-1";
        for cookie in &mut prepared.cookies {
            let mut entry = ffi::AuthData {
                protocol: cookie.protocol.as_ptr().cast_mut(), network: cookie.network.as_ptr().cast_mut(),
                name: auth_name.as_ptr().cast_mut(), length: 16, data: cookie.data.as_mut_ptr().cast(),
            };
            // SAFETY: IceSetPaAuthData copies the strings and cookie bytes.
            // This registration stays on the thread that owns libICE globals.
            unsafe { ffi::IceSetPaAuthData(1, &mut entry); }
        }

        Ok(Self { path: prepared.path.take().expect("prepared ICE authority path") })
    }
}

impl Drop for Authority {
    fn drop(&mut self) {
        let path = self.path.clone();
        // Server drops before its runtime; runtime shutdown drains this job
        // before the XSMP worker reports completion.
        tokio::task::spawn_blocking(move || { let _ = std::fs::remove_file(path); });
    }
}

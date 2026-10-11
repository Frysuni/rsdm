//! Authenticate compositor IPC peers and bound nonblocking event reads.

use std::{io::ErrorKind, os::fd::AsRawFd, path::Path, time::Duration};

use tokio::net::UnixStream;

use super::{SessionError, processes::ProcessHandle};

pub(super) const MAX_FRAME_BYTES: u64 = 1_048_576;

pub(super) async fn connect(path: &Path, uid: u32) -> Result<(UnixStream, u32, ProcessHandle), SessionError> {
    if !path.is_absolute() { return Err(SessionError::State("compositor socket path must be absolute".into())); }
    let stream = tokio::time::timeout(Duration::from_secs(2), UnixStream::connect(path)).await
        .map_err(|_| SessionError::State("connecting to compositor timed out".into()))??;
    let credentials = stream.peer_cred()?;
    if credentials.uid() != uid { return Err(SessionError::State("compositor socket belongs to another user".into())); }
    let pid = credentials.pid().and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid != 0)
        .ok_or_else(|| SessionError::State("compositor socket has no process identity".into()))?;
    let peer = ProcessHandle::socket_peer(&stream)?;
    if !peer.alive()? { return Err(SessionError::State("compositor socket owner has exited".into())); }
    Ok((stream, pid, peer))
}

pub(super) fn pending_events(stream: &UnixStream, mut events: Vec<u8>) -> Result<Vec<u8>, SessionError> {
    let mut buffer = [0_u8; 4096];
    loop {
        // Read kernel-queued events even before Tokio polls fresh readiness.
        // SAFETY: the socket FD and mutable buffer are valid for this call.
        let count = unsafe { libc::recv(stream.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len(), libc::MSG_DONTWAIT) };
        let read = if count < 0 { Err(std::io::Error::last_os_error()) } else { Ok(count as usize) };
        match read {
            Ok(0) => return Err(SessionError::State("compositor event stream disconnected before close".into())),
            Ok(count) => events.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(events),
            Err(error) => return Err(error.into()),
        }
        if events.len() as u64 > MAX_FRAME_BYTES {
            return Err(SessionError::State("compositor event burst is oversized".into()));
        }
    }
}

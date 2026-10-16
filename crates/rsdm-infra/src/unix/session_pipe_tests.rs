use std::{io::Read, os::{fd::AsRawFd, unix::net::UnixStream}, thread, time::Duration};

use super::*;

#[test]
fn nonblocking_partial_writes_preserve_the_complete_frame() {
    let (writer, mut reader) = UnixStream::pair().unwrap();
    writer.set_nonblocking(true).unwrap();
    reader.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let peer = thread::spawn(move || {
        let mut received = Vec::new();
        reader.read_to_end(&mut received).unwrap();
        received
    });
    let bytes: Vec<u8> = (0..1024 * 1024).map(|index| (index % 251) as u8).collect();
    let result = write_frame_until(writer.as_raw_fd(), &bytes, Instant::now() + Duration::from_secs(2));
    drop(writer);
    let received = peer.join().unwrap();
    result.unwrap();
    assert_eq!(received, bytes);
}

#[test]
fn expired_write_deadlines_do_not_send_a_frame() {
    let (writer, mut reader) = UnixStream::pair().unwrap();
    writer.set_nonblocking(true).unwrap();
    reader.set_nonblocking(true).unwrap();
    let error = write_frame_until(writer.as_raw_fd(), b"answer", Instant::now()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(reader.read(&mut [0; 1]).unwrap_err().kind(), io::ErrorKind::WouldBlock);
}

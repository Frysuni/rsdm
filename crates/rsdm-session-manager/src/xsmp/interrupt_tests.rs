use std::{io::{Read, Write}, os::{fd::AsRawFd, unix::net::UnixStream}, time::Duration};

use super::*;

#[tokio::test]
async fn stop_interrupts_blocking_socket_io_on_the_library_thread() {
    let interrupt = Arc::new(Interrupt::default());
    let (mut reader, _writer) = UnixStream::pair().unwrap();
    let socket = interrupt.register(reader.as_raw_fd()).unwrap();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (done, completed) = tokio::sync::oneshot::channel();
    let _ = std::thread::spawn(move || {
        let _socket = socket;
        let _ = entered.send(());
        let result = reader.read_exact(&mut [0_u8]);
        let _ = done.send(result);
    });
    started.await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    interrupt.stop();
    let error = tokio::time::timeout(Duration::from_millis(500), completed).await.unwrap().unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    interrupt.stopped().await;
}

#[tokio::test]
async fn stopping_does_not_shutdown_sockets_after_their_registration_is_released() {
    let interrupt = Interrupt::default();
    let (old, _old_writer) = UnixStream::pair().unwrap();
    let registration = interrupt.register(old.as_raw_fd()).unwrap();
    drop(registration);
    drop(old);
    let (mut reader, mut writer) = UnixStream::pair().unwrap();
    interrupt.stop();
    writer.write_all(b"live").unwrap();
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"live");
}

#[tokio::test]
async fn sockets_accepted_after_stop_are_interrupted_and_never_registered() {
    let interrupt = Interrupt::default();
    interrupt.stop();
    let (reader, mut writer) = UnixStream::pair().unwrap();
    assert!(interrupt.register(reader.as_raw_fd()).is_err());
    assert!(writer.write_all(b"late").is_err());
    assert!(interrupt.sockets.lock().unwrap().is_empty());
}

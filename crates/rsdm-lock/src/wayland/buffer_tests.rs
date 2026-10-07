use std::{io::{Read, Write}, os::unix::net::UnixStream, thread};

use smithay_client_toolkit::{
    reexports::client::{Connection, Proxy, globals::registry_queue_init},
    shm::Shm,
};

use super::*;

fn pool_fixture() -> (SlotPool, Connection, UnixStream) {
    let (client, server) = UnixStream::pair().unwrap();
    server.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let bootstrap = thread::spawn(move || advertise_shm(server));
    let connection = Connection::from_socket(client).unwrap();
    let (globals, queue) = registry_queue_init::<App>(&connection).unwrap();
    let shm = Shm::bind(&globals, &queue.handle()).unwrap();
    let pool = SlotPool::new(64, &shm).unwrap();
    (pool, connection, bootstrap.join().unwrap())
}

fn advertise_shm(mut server: UnixStream) -> UnixStream {
    let (_, opcode, args) = read_request(&mut server);
    assert_eq!(opcode, 1);
    let registry = args[0];
    let mut payload = 1u32.to_ne_bytes().to_vec();
    payload.extend_from_slice(&7u32.to_ne_bytes());
    payload.extend_from_slice(b"wl_shm\0\0");
    payload.extend_from_slice(&1u32.to_ne_bytes());
    send_event(&mut server, registry, 0, &payload);
    let (_, opcode, args) = read_request(&mut server);
    assert_eq!(opcode, 0);
    send_event(&mut server, args[0], 0, &0u32.to_ne_bytes());
    send_event(&mut server, 1, 1, &args[0].to_ne_bytes());
    server
}

fn send_event(server: &mut UnixStream, id: u32, opcode: u32, payload: &[u8]) {
    server.write_all(&id.to_ne_bytes()).unwrap();
    server.write_all(&(((payload.len() as u32 + 8) << 16) | opcode).to_ne_bytes()).unwrap();
    server.write_all(payload).unwrap();
}

fn read_request(server: &mut UnixStream) -> (u32, u32, Vec<u32>) {
    let mut header = [0; 8];
    server.read_exact(&mut header).unwrap();
    let id = u32::from_ne_bytes(header[..4].try_into().unwrap());
    let word = u32::from_ne_bytes(header[4..].try_into().unwrap());
    let mut payload = vec![0; (word >> 16) as usize - 8];
    server.read_exact(&mut payload).unwrap();
    let args = payload.chunks_exact(4)
        .map(|word| u32::from_ne_bytes(word.try_into().unwrap())).collect();
    (id, word & 0xffff, args)
}

fn release(connection: &Connection, server: &mut UnixStream, buffer: &Buffer) {
    connection.flush().unwrap();
    let guard = connection.prepare_read().unwrap();
    send_event(server, buffer.wl_buffer().id().protocol_id(), 0, &[]);
    guard.read().unwrap();
}

#[test]
fn compositor_release_makes_the_same_buffer_writable_again() {
    let (mut pool, connection, mut server) = pool_fixture();
    let mut buffers = Vec::new();
    assert_eq!(reusable_buffer(&mut buffers, &mut pool, 4, 4).unwrap(), Some(0));
    let original = buffers[0].wl_buffer().id();
    buffers[0].canvas(&mut pool).unwrap().fill(42);
    buffers[0].activate().unwrap();
    assert!(buffers[0].canvas(&mut pool).is_none());
    assert_eq!(reusable_buffer(&mut buffers, &mut pool, 4, 4).unwrap(), Some(1));
    release(&connection, &mut server, &buffers[0]);

    for _ in 0..100 {
        assert_eq!(reusable_buffer(&mut buffers, &mut pool, 4, 4).unwrap(), Some(0));
        assert_eq!(buffers[0].wl_buffer().id(), original);
        assert_eq!(buffers[0].canvas(&mut pool).unwrap(), &[42; 64]);
        assert_eq!(buffers.len(), 2);
    }
}

#[test]
fn busy_buffers_bound_allocation_even_across_configure_changes() {
    let (mut pool, connection, mut server) = pool_fixture();
    let mut buffers = Vec::new();
    reusable_buffer(&mut buffers, &mut pool, 4, 4).unwrap();
    buffers[0].activate().unwrap();
    reusable_buffer(&mut buffers, &mut pool, 4, 4).unwrap();
    buffers[1].activate().unwrap();
    let allocated = pool.len();
    let original_second = buffers[1].wl_buffer().id();
    for width in 1..1_000 {
        assert_eq!(reusable_buffer(&mut buffers, &mut pool, width, 2).unwrap(), None);
        assert_eq!(buffers.len(), 2);
        assert_eq!(pool.len(), allocated);
    }

    release(&connection, &mut server, &buffers[0]);
    assert_eq!(reusable_buffer(&mut buffers, &mut pool, 8, 2).unwrap(), Some(1));
    assert_eq!(buffers.len(), 2);
    assert_eq!(buffers[0].wl_buffer().id(), original_second);
    assert!(buffers[0].canvas(&mut pool).is_none());
    assert_eq!(buffers[1].stride(), 32);
    assert_eq!(buffers[1].height(), 2);
}

#[test]
fn released_obsolete_geometry_is_replaced_without_accumulating_buffers() {
    let (mut pool, _connection, _server) = pool_fixture();
    let mut buffers = Vec::new();
    for (width, height) in [(4, 4), (8, 2), (2, 8), (4, 4)] {
        assert_eq!(reusable_buffer(&mut buffers, &mut pool, width, height).unwrap(), Some(0));
        assert_eq!(buffers.len(), 1);
        assert_eq!(buffers[0].stride(), width as i32 * 4);
        assert_eq!(buffers[0].height(), height as i32);
        assert_eq!(pool.len(), 64);
    }
}

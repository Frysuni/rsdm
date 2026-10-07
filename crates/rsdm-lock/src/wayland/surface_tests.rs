use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    thread,
};

use smithay_client_toolkit::{output::OutputData, reexports::client::Proxy};

use super::*;

#[test]
fn output_removal_destroys_extensions_before_the_lock_surface() {
    let (connection, mut server, surface, ids, _lock) = create_surface(true);
    let mut surfaces = vec![surface];
    surfaces.retain(|_| false);
    connection.flush().unwrap();

    assert_eq!(destroy_requests(&mut server, 4), ids);
}

#[test]
fn a_pending_surface_clone_does_not_retain_extensions() {
    let (connection, mut server, surface, ids, _lock) = create_surface(true);
    let pending = surface.surface.clone();
    drop(surface);
    connection.flush().unwrap();
    assert_eq!(destroy_requests(&mut server, 2), ids[..2]);
    assert!(pending.wl_surface().is_alive());

    drop(pending);
    connection.flush().unwrap();
    assert_eq!(destroy_requests(&mut server, 2), ids[2..]);
}

#[test]
fn outputs_without_fractional_scaling_destroy_only_the_lock_surface() {
    let (connection, mut server, surface, ids, _lock) = create_surface(false);
    drop(surface);
    connection.flush().unwrap();

    assert_eq!(destroy_requests(&mut server, 2), ids);
}

fn create_surface(
    fractional: bool,
) -> (Connection, UnixStream, LockSurface, Vec<u32>, SessionLock) {
    let (client, server) = UnixStream::pair().unwrap();
    server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let bootstrap = thread::spawn(move || advertise_globals(server));
    let connection = Connection::from_socket(client).unwrap();
    let (globals, queue) = registry_queue_init::<App>(&connection).unwrap();
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).unwrap();
    let lock = SessionLockState::new(&globals, &qh).lock(&qh).unwrap();
    let output = globals.registry().bind(3, 1, &qh, OutputData::new(3));
    let viewporter: WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let manager: WpFractionalScaleManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh);
    let viewport = fractional.then(|| viewporter.get_viewport(&surface, &qh, ()));
    let scale = fractional.then(|| manager.get_fractional_scale(&surface, &qh, surface.clone()));
    let mut ids = Vec::new();
    if let (Some(scale), Some(viewport)) = (&scale, &viewport) {
        ids.extend([scale.id().protocol_id(), viewport.id().protocol_id()]);
    }
    let wl_surface_id = surface.id().protocol_id();
    let lock_surface = lock.create_lock_surface(surface, &output, &qh);
    let entry = LockSurface {
        output,
        output_name: "test-output".into(),
        surface: lock_surface,
        viewport,
        _fractional_scale: scale,
        scale_120: 120,
        preferred_scale_120: None,
        width: 0,
        height: 0,
        buffer: None,
        wallpaper_cache: None,
    };
    connection.flush().unwrap();
    let mut server = bootstrap.join().unwrap();
    let mut last = None;
    for _ in 0..(8 + if fractional { 2 } else { 0 }) {
        last = Some(read_request(&mut server));
    }
    ids.extend([last.unwrap().2[0], wl_surface_id]);
    (connection, server, entry, ids, lock)
}

fn destroy_requests(server: &mut UnixStream, count: usize) -> Vec<u32> {
    (0..count)
        .map(|_| {
            let (id, opcode, args) = read_request(server);
            assert_eq!(opcode, 0, "expected a protocol destroy request");
            assert!(args.is_empty());
            id
        })
        .collect()
}

fn advertise_globals(mut server: UnixStream) -> UnixStream {
    let (display, opcode, args) = read_request(&mut server);
    assert_eq!((display, opcode), (1, 1));
    let registry = args[0];
    for (name, interface) in [
        (1u32, "wl_compositor"), (2, "ext_session_lock_manager_v1"),
        (3, "wl_output"), (4, "wp_viewporter"), (5, "wp_fractional_scale_manager_v1"),
    ] {
        let mut payload = name.to_ne_bytes().to_vec();
        let length = interface.len() + 1;
        payload.extend_from_slice(&(length as u32).to_ne_bytes());
        payload.extend_from_slice(interface.as_bytes());
        payload.resize(8 + length.div_ceil(4) * 4, 0);
        payload.extend_from_slice(&1u32.to_ne_bytes());
        send_event(&mut server, registry, 0, &payload);
    }
    let (display, opcode, args) = read_request(&mut server);
    assert_eq!((display, opcode), (1, 0));
    send_event(&mut server, args[0], 0, &0u32.to_ne_bytes());
    send_event(&mut server, 1, 1, &args[0].to_ne_bytes());
    server
}

fn send_event(server: &mut UnixStream, id: u32, opcode: u32, payload: &[u8]) {
    server.write_all(&id.to_ne_bytes()).unwrap();
    let header = ((payload.len() as u32 + 8) << 16) | opcode;
    server.write_all(&header.to_ne_bytes()).unwrap();
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

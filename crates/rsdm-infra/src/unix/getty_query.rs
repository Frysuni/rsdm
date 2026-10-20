//! Discover the distro's own console login for the TTY fallback.
//!
//! systemd exposes `ExecStart` as a typed array of commands over D-Bus. Using
//! that property preserves argv boundaries; the textual `systemctl show`
//! representation cannot do so reliably.

use std::time::Duration;

use zbus::{
    Connection, Proxy, connection::Builder,
    OwnedObjectPath,
};

// Executable, argv, ignore-errors, start/exit realtime and monotonic timestamps,
// PID, exit code and exit status: systemd's a(sasbttttuii) property.
type ExecCommand = (String, Vec<String>, bool, u64, u64, u64, u64, u32, i32, i32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GettyCommand {
    pub executable: String,
    pub argv: Vec<String>,
}

/// Resolve the console login systemd runs on `tty_path`, or `None` when it
/// cannot be determined. The executable may differ from argv[0].
pub async fn discover_system_getty(tty_path: &str) -> Option<GettyCommand> {
    let tty = tty_path.strip_prefix("/dev/").unwrap_or(tty_path);
    let builder = Builder::system()
        .method_timeout(Duration::from_secs(5));
    let connection = tokio::time::timeout(Duration::from_secs(5), builder.build()).await.ok()?.ok()?;
    for unit in [format!("getty@{tty}.service"), "getty@.service".to_string()] {
        let Some(command) = query_exec_start(&connection, &unit).await else {
            continue;
        };
        if !command.argv.is_empty() {
            tracing::info!(unit, ?command, "resolved system getty for the TTY fallback");
            return Some(command);
        }
    }
    None
}

async fn query_exec_start(connection: &Connection, unit: &str) -> Option<GettyCommand> {
    let manager = Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await.ok()?;
    let path: OwnedObjectPath = manager.call("GetUnit", &(unit,)).await.ok()?;
    let service = Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        path,
        "org.freedesktop.systemd1.Service",
    )
    .await.ok()?;
    let commands: Vec<ExecCommand> = service.get_property("ExecStart").await.ok()?;
    // systemd already expanded unit specifiers while loading the command.
    // Percent signs remaining in these arguments are literal data.
    commands.into_iter().next().map(|(executable, argv, ..)| GettyCommand { executable, argv })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Manager;

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl Manager {
        fn get_unit(&self, unit: &str) -> OwnedObjectPath {
            assert_eq!(unit, "getty@tty3.service");
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/getty").unwrap()
        }
    }

    struct Getty(&'static str);

    // systemd's published ExecStart wire format, independent of the client.
    type SystemdExecStart = (String, Vec<String>, bool, u64, u64, u64, u64, u32, i32, i32);

    #[zbus::interface(name = "org.freedesktop.systemd1.Service")]
    impl Getty {
        #[zbus(property)]
        fn exec_start(&self) -> Vec<SystemdExecStart> {
            let argv = [self.0, "-o", "-p -- \\u", "", "tty3", "%", "%I", "%%"]
                .into_iter().map(str::to_string).collect();
            vec![("/sbin/agetty".into(), argv, false, 1, 2, 3, 4, 1234, 1, 0)]
        }
    }

    #[tokio::test]
    async fn reads_typed_exec_start_without_retokenizing_arguments() {
        assert_eq!(<Vec<SystemdExecStart> as zbus::Type>::SIGNATURE.to_string(), "a(sasbttttuii)");
        for argv0 in ["/sbin/agetty", "custom-getty"] {
            let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
            let server = Builder::unix_stream(server_socket.into_std().unwrap()).p2p()
                .server(zbus::Guid::generate())
                .serve_at("/org/freedesktop/systemd1", Manager)
                .serve_at("/org/freedesktop/systemd1/unit/getty", Getty(argv0));
            let client = Builder::unix_stream(client_socket.into_std().unwrap()).p2p();
            let (server, client) = tokio::join!(server.build(), client.build());
            let _server = server.unwrap();
            let client = client.unwrap();

            let command = query_exec_start(&client, "getty@tty3.service").await.unwrap();
            assert_eq!(command.executable, "/sbin/agetty");
            assert_eq!(command.argv, [argv0, "-o", "-p -- \\u", "", "tty3", "%", "%I", "%%"]);
        }
    }
}

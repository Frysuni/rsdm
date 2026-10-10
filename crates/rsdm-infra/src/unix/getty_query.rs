//! Discover the distro's own console login for the TTY fallback.
//!
//! systemd exposes `ExecStart` as a typed array of commands over D-Bus. Using
//! that property preserves argv boundaries; the textual `systemctl show`
//! representation cannot do so reliably.

use std::time::Duration;

use zbus::{
    Connection, Proxy, connection::Builder,
    zvariant::OwnedObjectPath,
};

type ExecCommand = (String, Vec<String>, bool, u64, u64, i32, u32);

/// Resolve the console login systemd runs on `tty_path`, or `None` when it
/// cannot be determined. The result is a ready-to-exec argv.
pub async fn discover_system_getty(tty_path: &str) -> Option<Vec<String>> {
    let tty = tty_path.strip_prefix("/dev/").unwrap_or(tty_path);
    let builder = Builder::system()
        .ok()?
        .method_timeout(Duration::from_secs(5));
    let connection = tokio::time::timeout(Duration::from_secs(5), builder.build()).await.ok()?.ok()?;
    for unit in [format!("getty@{tty}.service"), "getty@.service".to_string()] {
        let Some(argv) = query_exec_start(&connection, &unit, tty).await else {
            continue;
        };
        if !argv.is_empty() {
            tracing::info!(unit, ?argv, "resolved system getty for the TTY fallback");
            return Some(argv);
        }
    }
    None
}

async fn query_exec_start(connection: &Connection, unit: &str, tty: &str) -> Option<Vec<String>> {
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
    commands.into_iter().next().map(|(_, argv, _, _, _, _, _)| resolve_argv(&argv, tty))
}

fn resolve_argv(argv: &[String], tty: &str) -> Vec<String> {
    argv.iter().map(|arg| substitute_specifiers(arg, tty)).collect()
}

fn substitute_specifiers(value: &str, tty: &str) -> String {
    value.replace("%I", tty).replace("%%", "%")
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

    struct Getty;

    #[zbus::interface(name = "org.freedesktop.systemd1.Service")]
    impl Getty {
        #[zbus(property)]
        fn exec_start(&self) -> Vec<ExecCommand> {
            let argv = ["/sbin/agetty", "-o", "-p -- \\u", "", "%I", "%%"]
                .into_iter().map(str::to_string).collect();
            vec![("/sbin/agetty".into(), argv, false, 0, 0, 0, 0)]
        }
    }

    #[tokio::test]
    async fn reads_typed_exec_start_without_retokenizing_arguments() {
        let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
        let server = Builder::unix_stream(server_socket).p2p()
            .server(zbus::Guid::generate()).unwrap()
            .serve_at("/org/freedesktop/systemd1", Manager).unwrap()
            .serve_at("/org/freedesktop/systemd1/unit/getty", Getty).unwrap();
        let client = Builder::unix_stream(client_socket).p2p();
        let (server, client) = tokio::join!(server.build(), client.build());
        let _server = server.unwrap();
        let client = client.unwrap();

        let argv = query_exec_start(&client, "getty@tty3.service", "tty3").await.unwrap();
        assert_eq!(argv, ["/sbin/agetty", "-o", "-p -- \\u", "", "tty3", "%"]);
    }

    #[test]
    fn substitutes_template_instance_without_retokenizing_argv() {
        let argv = vec![
            "/sbin/agetty".to_string(),
            "-o".to_string(),
            "-p -- \\u".to_string(),
            "%I".to_string(),
        ];
        assert_eq!(
            resolve_argv(&argv, "tty3"),
            vec!["/sbin/agetty", "-o", "-p -- \\u", "tty3"]
        );
    }

    #[test]
    fn preserves_typed_empty_and_percent_arguments() {
        let argv = vec!["agetty".to_string(), String::new(), "%%".to_string()];
        assert_eq!(resolve_argv(&argv, "tty1"), vec!["agetty", "", "%"]);
    }
}

//! Discover the distro's own console login for the TTY fallback.
//!
//! systemd exposes `ExecStart` as a typed array of commands over D-Bus. Using
//! that property preserves argv boundaries; the textual `systemctl show`
//! representation cannot do so reliably.

use std::time::Duration;

use zbus::{
    blocking::{Connection, Proxy, connection::Builder},
    zvariant::OwnedObjectPath,
};

type ExecCommand = (String, Vec<String>, bool, u64, u64, i32, u32);

/// Resolve the console login systemd runs on `tty_path`, or `None` when it
/// cannot be determined. The result is a ready-to-exec argv.
pub fn discover_system_getty(tty_path: &str) -> Option<Vec<String>> {
    let tty = tty_path.strip_prefix("/dev/").unwrap_or(tty_path);
    let connection = Builder::system()
        .ok()?
        .method_timeout(Duration::from_secs(5))
        .build()
        .ok()?;
    for unit in [format!("getty@{tty}.service"), "getty@.service".to_string()] {
        let Some(argv) = query_exec_start(&connection, &unit, tty) else {
            continue;
        };
        if !argv.is_empty() {
            tracing::info!(unit, ?argv, "resolved system getty for the TTY fallback");
            return Some(argv);
        }
    }
    None
}

fn query_exec_start(connection: &Connection, unit: &str, tty: &str) -> Option<Vec<String>> {
    let manager = Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .ok()?;
    let path: OwnedObjectPath = manager.call("GetUnit", &(unit,)).ok()?;
    let service = Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        path,
        "org.freedesktop.systemd1.Service",
    )
    .ok()?;
    let commands: Vec<ExecCommand> = service.get_property("ExecStart").ok()?;
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

//! Stable application identities and unique systemd service instances.

use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use super::systemd::ManagedUnitKind;

static NEXT_UNIT_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn unique_unit_name(kind: ManagedUnitKind, program: &str) -> String {
    let id = NEXT_UNIT_ID.fetch_add(1, Ordering::Relaxed);
    // Fresh app processes all start their counter at zero, so PID alone can
    // collide after reuse.
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let instance = format!("{}-{millis}-{id}", std::process::id());
    let name = sanitize(program);
    match kind {
        ManagedUnitKind::Session => format!("session-rsdm-{name}-{instance}"),
        // Portals derive the app ID from the service name. Keep the changing
        // instance after @ and avoid ambiguous dashes in the application ID.
        ManagedUnitKind::App => format!("app-rsdm-{}@{instance}", name.replace('-', "_")),
    }
}

fn sanitize(program: &str) -> String {
    let stem = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    let mut sanitized = String::new();
    let mut previous_dash = false;
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            sanitized.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            sanitized.push('-');
            previous_dash = true;
        }
    }

    let sanitized = sanitized.trim_matches('-');
    if sanitized.is_empty() {
        "app".to_string()
    } else {
        sanitized.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_makes_safe_unit_names() {
        assert_eq!(sanitize("/usr/bin/niri-session"), "niri-session");
        assert_eq!(sanitize("foo/bar baz"), "bar-baz");
        assert_eq!(sanitize("////"), "app");
    }

    #[test]
    fn app_identity_is_stable_across_unique_instances() {
        for (program, expected) in [
            ("/usr/bin/obs", "app-rsdm-obs"),
            ("/usr/bin/my-player", "app-rsdm-my_player"),
        ] {
            let first = unique_unit_name(ManagedUnitKind::App, program);
            let second = unique_unit_name(ManagedUnitKind::App, program);
            assert_ne!(first, second);
            assert_eq!(first.split_once('@').unwrap().0, expected);
            assert_eq!(second.split_once('@').unwrap().0, expected);
        }
    }
}

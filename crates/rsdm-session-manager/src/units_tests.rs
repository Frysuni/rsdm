use std::{fs, sync::atomic::{AtomicUsize, Ordering}};

use zbus::zvariant::OwnedValue;

use super::*;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct Programs(PathBuf);

impl Programs {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rsdm-unit-test-{}-{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        for name in ["example", "true"] {
            let program = path.join(name);
            fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(program, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(path)
    }

    fn environment(&self) -> Vec<(String, String)> {
        vec![("PATH".into(), self.0.to_string_lossy().into_owned())]
    }
}

impl Drop for Programs {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn property(properties: &UnitProperties, name: &str) -> OwnedValue {
    properties.iter().find(|(key, _)| *key == name).unwrap().1.try_to_owned().unwrap()
}

#[test]
fn exec_properties_keep_literal_arguments_and_use_the_callers_path() {
    let programs = Programs::new();
    let argv = vec!["example".into(), "$HOME".into(), "two words".into(), "".into()];
    let properties = command_properties(&argv, &programs.environment(), &programs.0).unwrap();
    let commands: Vec<(String, Vec<String>, Vec<String>)> = property(&properties, "ExecStartEx").try_into().unwrap();

    assert_eq!(commands[0].0, programs.0.join("example").to_string_lossy());
    assert_eq!(commands[0].1, argv);
    assert_eq!(commands[0].2, ["no-env-expand"]);
    let absent: Vec<String> = property(&properties, "UnsetEnvironment").try_into().unwrap();
    assert_eq!(absent, ["WAYLAND_DISPLAY", "DISPLAY", "XAUTHORITY"]);
}

#[test]
fn relative_search_paths_are_resolved_against_the_callers_directory() {
    let programs = Programs::new();
    let environment = vec![("PATH".into(), ".".into())];
    assert_eq!(resolve_executable("example", &environment, &programs.0).unwrap(), programs.0.join(".").join("example"));
}

#[test]
fn anchor_does_not_create_an_ordering_cycle_through_autostart_apps() {
    let programs = Programs::new();
    let properties = anchor_properties(Some("example-compositor.service"), &programs.environment(), &programs.0).unwrap();
    let after: Vec<String> = property(&properties, "After").try_into().unwrap();
    assert!(!after.contains(&AUTOSTART_TARGET.to_string()));
    assert!(after.contains(&SESSION_TARGET.to_string()));
}

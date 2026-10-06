use super::*;

fn contains_arg(args: &[String], expected: &str) -> bool {
    args.iter().any(|arg| arg == expected)
}

fn contains_property(args: &[String], expected: &str) -> bool {
    contains_arg(args, &format!("--property={expected}"))
}

fn slice_arg(args: &[String]) -> Option<&str> {
    args.windows(2)
        .find(|window| window[0] == "--slice")
        .map(|window| window[1].as_str())
}

#[test]
fn session_unit_tracks_the_compositor_main_process() {
    let args = systemd_run_args(
        ManagedUnitKind::Session,
        "session-rsdm-test",
        &[("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string())],
    );

    assert!(contains_arg(&args, "--user"));
    assert!(contains_arg(&args, "--collect"));
    assert!(contains_arg(&args, "--same-dir"));
    assert!(contains_arg(&args, "--expand-environment=no"));
    assert!(contains_arg(&args, "--wait"));
    assert!(!contains_arg(&args, "--scope"));
    assert_eq!(slice_arg(&args), Some(SESSION_SLICE));
    assert!(contains_property(&args, "Type=exec"));
    assert!(contains_property(&args, "ExitType=main"));
    assert!(contains_property(&args, "KillMode=control-group"));
    assert!(contains_property(
        &args,
        &format!("PartOf={SESSION_TARGET}")
    ));
    assert!(contains_property(&args, &format!("After={PRE_TARGET}")));
    assert!(contains_property(&args, &format!("Wants={PRE_TARGET}")));
    assert!(contains_arg(&args, "--setenv=WAYLAND_DISPLAY"));
    assert!(contains_property(
        &args,
        "UnsetEnvironment=DISPLAY XAUTHORITY"
    ));
    assert!(!args.iter().any(|arg| arg.contains("wayland-1")));
    assert_eq!(args.last().map(String::as_str), Some("--"));
}

#[test]
fn xwayland_app_keeps_its_credentials_without_inheriting_a_wayland_display() {
    let args = systemd_run_args(
        ManagedUnitKind::App,
        "app-rsdm-test",
        &[
            ("DISPLAY".into(), ":2".into()),
            ("XAUTHORITY".into(), "/run/user/1000/xauth".into()),
            ("XCURSOR_THEME".into(), "Adwaita".into()),
            ("XCURSOR_SIZE".into(), "32".into()),
        ],
    );
    assert!(contains_property(&args, "UnsetEnvironment=WAYLAND_DISPLAY"));
    for name in ["DISPLAY", "XAUTHORITY", "XCURSOR_THEME", "XCURSOR_SIZE"] {
        assert!(contains_arg(&args, &format!("--setenv={name}")));
    }
}

#[test]
fn app_unit_lives_until_its_cgroup_is_empty() {
    let args = systemd_run_args(
        ManagedUnitKind::App,
        "app-rsdm-test",
        &[
            ("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string()),
            ("bad-name".to_string(), "ignored".to_string()),
        ],
    );

    assert!(contains_arg(&args, "--user"));
    assert!(contains_arg(&args, "--collect"));
    assert!(contains_arg(&args, "--same-dir"));
    assert!(contains_arg(&args, "--expand-environment=no"));
    assert!(!contains_arg(&args, "--wait"));
    assert!(!contains_arg(&args, "--scope"));
    assert_eq!(slice_arg(&args), Some(APP_SLICE));
    assert!(contains_property(&args, "Type=exec"));
    assert!(contains_property(&args, "ExitType=cgroup"));
    assert!(contains_property(&args, "KillMode=control-group"));
    assert!(contains_property(
        &args,
        &format!("PartOf={SESSION_TARGET}")
    ));
    assert!(contains_property(&args, &format!("After={SESSION_TARGET}")));
    assert!(contains_property(
        &args,
        &format!("Requisite={SESSION_TARGET}")
    ));
    assert!(contains_arg(&args, "--setenv=WAYLAND_DISPLAY"));
    assert!(!contains_arg(&args, "--setenv=bad-name"));
    assert!(!args.iter().any(|arg| arg.contains("wayland-1")));
    assert_eq!(args.last().map(String::as_str), Some("--"));
}

#[test]
fn teardown_stops_targets_not_the_app_slice_directly() {
    let mut cfg = SessionManagerConfig::default();
    cfg.extra_env.push("EXTRA_ENV".to_string());
    cfg.extra_env.push("bad-name".to_string());

    let commands = anchored_teardown_commands(&cfg);

    assert!(
        commands
            .iter()
            .any(|command| command == &vec!["stop".to_string(), ANCHOR_UNIT.to_string()])
    );
    assert!(
        commands
            .iter()
            .any(|command| command == &vec!["stop".to_string(), SESSION_TARGET.to_string()])
    );
    assert!(
        commands
            .iter()
            .any(|command| command == &vec!["stop".to_string(), PRE_TARGET.to_string()])
    );
    assert!(
        !commands
            .iter()
            .any(|command| command == &vec!["stop".to_string(), APP_SLICE.to_string()])
    );
    let unset = commands
        .iter()
        .find(|command| command.first().map(String::as_str) == Some("unset-environment"))
        .expect("teardown unsets the exported session environment");
    assert!(unset.iter().any(|arg| arg == "WAYLAND_DISPLAY"));
    assert!(unset.iter().any(|arg| arg == "EXTRA_ENV"));
    assert!(!unset.iter().any(|arg| arg == "bad-name"));
}

#[test]
fn anchor_binds_to_the_compositor_unit_when_known() {
    let args = anchor_args(Some("session-rsdm-niri-1-2-3.service"));
    assert!(contains_property(
        &args,
        "BindsTo=session-rsdm-niri-1-2-3.service"
    ));
    assert!(contains_property(
        &args,
        "After=session-rsdm-niri-1-2-3.service"
    ));
    assert!(contains_property(&args, &format!("Wants={SESSION_TARGET}")));
    assert!(contains_property(
        &args,
        &format!("Wants={AUTOSTART_TARGET}")
    ));
    assert!(contains_property(&args, "RemainAfterExit=yes"));
    assert!(contains_arg(&args, "--collect"));
    assert_eq!(args.last().map(String::as_str), Some("true"));
}

#[test]
fn anchor_without_a_compositor_unit_has_no_binds_to() {
    let args = anchor_args(None);
    assert!(!args.iter().any(|arg| arg.contains("BindsTo=")));
    assert!(contains_property(&args, &format!("Wants={SESSION_TARGET}")));
}

#[test]
fn releasing_the_anchor_never_stops_a_compositor_owned_target() {
    // When the compositor owns graphical-session.target rsdm must only touch
    // its own anchor unit, never the shared targets.
    let commands = release_anchor_commands();
    assert!(
        commands.iter().all(|command| !command
            .iter()
            .any(|arg| arg == SESSION_TARGET || arg == PRE_TARGET || arg == AUTOSTART_TARGET)),
        "anchor release must not stop any graphical-session target"
    );
    assert!(
        commands
            .iter()
            .any(|command| command == &vec!["stop".to_string(), ANCHOR_UNIT.to_string()])
    );
    assert!(commands.iter().any(|command| {
        command == &vec!["reset-failed".to_string(), ANCHOR_UNIT.to_string()]
    }));
}

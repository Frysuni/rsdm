use super::*;
use crate::surface::testing::VecSurface;
use rsdm_core::domain::{DesignConfig, Session, ThemePreset, theme};

fn design() -> Design {
    Design::from_config(&DesignConfig::default())
}

fn sessions() -> Vec<Session> {
    vec![Session::new("niri", "Niri", "niri-session", "/x")]
}

#[test]
fn login_renders_title_and_fields() {
    let sessions = sessions();
    let scene = LoginScene {
        title: banner::title_lines(&design(), None),
        hostname: Some("host".into()),
        clock: Some("12:00".into()),
        username: "alice",
        password_preview: "***".into(),
        field: Field::Password,
        pending: None,
        console_exit_enabled: false,
        message: None,
        message_is_error: false,
        sessions: &sessions,
        selected: 0,
        session_field_visible: false,
        session_picker_enabled: false,
        picker_open: false,
    };
    let mut s = VecSurface::new(100, 32);
    render_login(&mut s, &design(), &scene, &Menu::new(), 0);
    let dump = s.dump();
    assert!(dump.contains("User"));
    assert!(dump.contains("Password"));
    assert!(dump.contains("F11"));
    assert!(dump.contains("Menu"));
    assert!(dump.contains("Field"));
    assert!(!dump.contains("Exit"));
}

#[test]
fn tiny_surface_shows_notice() {
    let sessions = sessions();
    let scene = LoginScene {
        title: vec!["RSDM".into()],
        hostname: None,
        clock: None,
        username: "",
        password_preview: String::new(),
        field: Field::Username,
        pending: None,
        console_exit_enabled: false,
        message: None,
        message_is_error: false,
        sessions: &sessions,
        selected: 0,
        session_field_visible: false,
        session_picker_enabled: false,
        picker_open: false,
    };
    let mut s = VecSurface::new(20, 6);
    render_login(&mut s, &design(), &scene, &Menu::new(), 0);
    assert!(s.dump().contains("too small"));
}

#[test]
fn lock_renders_username_and_caption() {
    let scene = LockScene {
        title: vec!["RSDM".into()],
        hostname: Some("host".into()),
        clock: Some("12:00".into()),
        username: "alice",
        password_preview: "****".into(),
        message: Some("incorrect password"),
        message_is_error: true,
        pending: None,
        hibernate_available: true,
    };
    let mut s = VecSurface::new(100, 32);
    let d = design();
    render_lock(&mut s, &d, &scene, &Menu::new(), 0);
    let dump = s.dump();
    assert!(dump.contains("LOCKED"));
    assert!(dump.contains("alice"));
    assert!(dump.contains("Unlock"));
    assert!(dump.contains("Hibernate"));
    assert!(dump.contains("Sleep"));
    let _ = theme::palette(ThemePreset::Nord);
}

#[test]
fn lock_too_small_keeps_a_usable_prompt() {
    let scene = LockScene {
        title: vec!["RSDM".into()],
        hostname: None,
        clock: None,
        username: "alice",
        password_preview: "***".into(),
        message: None,
        message_is_error: false,
        pending: None,
        hibernate_available: false,
    };
    let mut s = VecSurface::new(20, 6);
    render_lock(&mut s, &design(), &scene, &Menu::new(), 0);
    let dump = s.dump();
    assert!(
        dump.contains("Password"),
        "password prompt must remain usable"
    );
    assert!(dump.contains("too small"));
}

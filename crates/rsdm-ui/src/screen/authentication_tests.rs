use super::*;
use crate::surface::testing::VecSurface;
use rsdm_core::domain::{BorderStyle, DesignConfig};

fn login(message: &str) -> LoginScene<'_> {
    LoginScene {
        title: vec!["RSDM".into(); 10],
        hostname: None,
        clock: None,
        username: "alice",
        password_preview: "123".into(),
        authentication_active: true,
        field: Field::Password,
        pending: None,
        console_exit_enabled: false,
        message: Some(message),
        message_is_error: false,
        sessions: &[],
        selected: 0,
        session_field_visible: true,
        session_picker_enabled: false,
        picker_open: false,
    }
}

fn lock(message: &str) -> LockScene<'_> {
    LockScene {
        title: vec!["RSDM".into(); 10],
        hostname: None,
        clock: None,
        username: "alice",
        password_preview: "123".into(),
        authentication_active: true,
        message: Some(message),
        message_is_error: false,
        pending: None,
        hibernate_available: false,
        suspend_available: false,
    }
}

fn styles() -> [BorderStyle; 10] {
    [
        BorderStyle::None, BorderStyle::Minimal, BorderStyle::Modern,
        BorderStyle::Wave, BorderStyle::Classic, BorderStyle::Pulse,
        BorderStyle::Ascii1, BorderStyle::Ascii2, BorderStyle::Ascii3,
        BorderStyle::Ascii4,
    ]
}

#[test]
fn current_prompt_and_response_survive_multiline_notices() {
    let mut message: String = (0..20).map(|i| format!("notice {i}\n")).collect();
    message.push_str("OTP:");
    for style in styles() {
        let mut design = Design::from_config(&DesignConfig::default());
        design.border = style;
        for (width, height) in [(80, 24), (44, 16), (20, 6)] {
            let mut surface = VecSurface::new(width, height);
            render_login(&mut surface, &design, &login(&message), &Menu::new(), 0);
            assert_prompt(&surface, "Greeter", style);
            surface.clear(design.palette().bg_base);
            render_lock(&mut surface, &design, &lock(&message), &Menu::new(), 0);
            assert_prompt(&surface, "Lock", style);
        }
    }
}

fn assert_prompt(surface: &VecSurface, front: &str, style: BorderStyle) {
    let dump = surface.dump();
    assert!(dump.contains("Response"), "{front} {style:?}:\n{dump}");
    assert!(dump.contains("123_"), "{front} {style:?}:\n{dump}");
    assert!(dump.contains("OTP:"), "{front} {style:?}:\n{dump}");
    assert!(!dump.contains("notice 0"), "earlier notices should give way to the prompt");
}

#[test]
fn current_prompt_survives_wrapped_notices() {
    let message = format!("{}\nOTP:", "a long notice ".repeat(200));
    let design = Design::from_config(&DesignConfig::default());
    let mut surface = VecSurface::new(44, 16);
    render_login(&mut surface, &design, &login(&message), &Menu::new(), 0);
    assert_prompt(&surface, "Greeter", design.border);
    surface.clear(design.palette().bg_base);
    render_lock(&mut surface, &design, &lock(&message), &Menu::new(), 0);
    assert_prompt(&surface, "Lock", design.border);
}

#[test]
fn short_authentication_messages_keep_the_banner_when_it_fits() {
    let design = Design::from_config(&DesignConfig::default());
    let mut login = login("OTP:");
    login.title = vec!["RSDM".into()];
    let mut lock = lock("OTP:");
    lock.title = vec!["RSDM".into()];
    let mut surface = VecSurface::new(100, 40);
    render_login(&mut surface, &design, &login, &Menu::new(), 0);
    let dump = surface.dump();
    assert!(dump.contains("RSDM"));
    assert!(dump.contains("Response"));
    assert!(dump.contains("OTP:"));
    surface.clear(design.palette().bg_base);
    render_lock(&mut surface, &design, &lock, &Menu::new(), 0);
    let dump = surface.dump();
    assert!(dump.contains("RSDM"));
    assert!(dump.contains("Response"));
    assert!(dump.contains("OTP:"));
}

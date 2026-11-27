static TERMINATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

use super::*;
use crossterm::event::{KeyEvent, KeyModifiers};
use rsdm_core::domain::{AppConfig, Session};

fn sessions() -> Vec<Session> {
    vec![Session::new("niri", "Niri", "niri-session", "/x")]
}

fn press(form: &mut FormState, model: &LoginUiModel<'_>, code: KeyCode) -> Option<FormEvent> {
    handle_key(KeyEvent::new(code, KeyModifiers::NONE), form, model)
}

#[test]
fn destructive_actions_require_a_confirming_second_press() {
    let mut config = AppConfig::default();
    config.security.deny_root = false;
    let sessions = sessions();
    let model = LoginUiModel {
        config: &config,
        sessions: &sessions,
        remembered_username: None,
        remembered_session: None,
        error_message: None,
        terminate: &TERMINATE,
    };
    let mut form = FormState::new(&model);

    assert!(press(&mut form, &model, KeyCode::Esc).is_none());
    assert_eq!(form.pending, Some(Pending::Exit));
    assert!(matches!(
        press(&mut form, &model, KeyCode::Esc),
        Some(FormEvent::Ui(LoginUiEvent::ExitToTty))
    ));
    assert_eq!(form.pending, None);

    assert!(press(&mut form, &model, KeyCode::F(11)).is_none());
    assert!(matches!(
        press(&mut form, &model, KeyCode::F(11)),
        Some(FormEvent::Ui(LoginUiEvent::Reboot))
    ));

    assert!(press(&mut form, &model, KeyCode::F(12)).is_none());
    assert!(matches!(
        press(&mut form, &model, KeyCode::F(12)),
        Some(FormEvent::Ui(LoginUiEvent::Shutdown))
    ));
}

#[test]
fn a_different_key_cancels_and_is_swallowed() {
    let mut config = AppConfig::default();
    config.security.deny_root = false;
    let sessions = sessions();
    let model = LoginUiModel {
        config: &config,
        sessions: &sessions,
        remembered_username: None,
        remembered_session: None,
        error_message: None,
        terminate: &TERMINATE,
    };
    let mut form = FormState::new(&model);

    assert!(press(&mut form, &model, KeyCode::Esc).is_none());
    assert_eq!(form.pending, Some(Pending::Exit));
    assert!(press(&mut form, &model, KeyCode::Char('a')).is_none());
    assert_eq!(form.pending, None);
    assert!(form.username.is_empty());
    assert!(press(&mut form, &model, KeyCode::Esc).is_none());
    assert_eq!(form.pending, Some(Pending::Exit));
}

#[test]
fn a_different_action_key_cancels_and_is_swallowed() {
    let mut config = AppConfig::default();
    config.security.deny_root = false;
    let sessions = sessions();
    let model = LoginUiModel {
        config: &config,
        sessions: &sessions,
        remembered_username: None,
        remembered_session: None,
        error_message: None,
        terminate: &TERMINATE,
    };
    let mut form = FormState::new(&model);

    assert!(press(&mut form, &model, KeyCode::Esc).is_none());
    assert!(press(&mut form, &model, KeyCode::F(12)).is_none());
    assert_eq!(form.pending, None);
}

#[test]
fn enter_advances_to_the_first_empty_field_instead_of_submitting() {
    let config = AppConfig::default();
    let sessions = sessions();
    let model = LoginUiModel {
        config: &config,
        sessions: &sessions,
        remembered_username: None,
        remembered_session: None,
        error_message: None,
        terminate: &TERMINATE,
    };
    let mut form = FormState::new(&model);
    assert_eq!(form.field, Field::Username);

    assert!(press(&mut form, &model, KeyCode::Enter).is_none());
    assert_eq!(form.field, Field::Username);

    form.username.push_str("alice");
    assert!(press(&mut form, &model, KeyCode::Enter).is_none());
    assert_eq!(form.field, Field::Password);

    form.password.push_str("hunter2");
    assert!(matches!(
        press(&mut form, &model, KeyCode::Enter),
        Some(FormEvent::Submit(_))
    ));
}

#[test]
fn arrows_move_between_fields() {
    let config = AppConfig::default();
    let sessions = sessions();
    let model = LoginUiModel {
        config: &config,
        sessions: &sessions,
        remembered_username: None,
        remembered_session: None,
        error_message: None,
        terminate: &TERMINATE,
    };
    let mut form = FormState::new(&model);
    assert_eq!(form.field, Field::Username);
    press(&mut form, &model, KeyCode::Down);
    assert_eq!(form.field, Field::Password);
    press(&mut form, &model, KeyCode::Up);
    assert_eq!(form.field, Field::Username);
}

#[test]
fn enter_on_an_empty_password_starts_pam_authentication() {
    let config = AppConfig::default();
    let sessions = sessions();
    let model = LoginUiModel {
        config: &config,
        sessions: &sessions,
        remembered_username: Some("alice"),
        remembered_session: None,
        error_message: None,
        terminate: &TERMINATE,
    };
    let mut form = FormState::new(&model);
    let Some(FormEvent::Submit(attempt)) = press(&mut form, &model, KeyCode::Enter) else {
        panic!("PAM must decide whether an empty initial password is acceptable");
    };
    assert_eq!(attempt.username, "alice");
    assert_eq!(attempt.session_id, "niri");
    assert!(attempt.password.expose_secret().is_empty());
}

#[test]
fn enter_while_armed_cancels_and_is_swallowed() {
    let config = AppConfig::default();
    let sessions = sessions();
    let model = LoginUiModel {
        config: &config,
        sessions: &sessions,
        remembered_username: Some("alice"),
        remembered_session: None,
        error_message: None,
        terminate: &TERMINATE,
    };
    let mut form = FormState::new(&model);
    form.password.push_str("hunter2");

    assert!(press(&mut form, &model, KeyCode::F(11)).is_none());
    assert!(press(&mut form, &model, KeyCode::Enter).is_none());
    assert_eq!(form.pending, None);
    assert_eq!(&*form.password, "hunter2");
}

#[test]
fn session_picker_and_power_confirmation_do_not_leak_keys_into_a_login_attempt() {
    let config = AppConfig::default();
    let sessions = [
        Session::new("niri", "Niri", "niri-session", "/niri.desktop"),
        Session::new("sway", "Sway", "sway", "/sway.desktop"),
        Session::new("labwc", "Labwc", "labwc", "/labwc.desktop"),
    ];
    let model = LoginUiModel {
        config: &config, sessions: &sessions,
        remembered_username: Some("alice"), remembered_session: Some("sway"),
        error_message: None, terminate: &TERMINATE,
    };
    let mut form = FormState::new(&model);
    for ch in "pässword".chars() {
        assert!(press(&mut form, &model, KeyCode::Char(ch)).is_none());
    }
    assert!(press(&mut form, &model, KeyCode::F(2)).is_none());
    assert!(form.modal_open);
    for key in [KeyCode::Down, KeyCode::Down, KeyCode::F(12), KeyCode::Char('x'), KeyCode::Enter] {
        assert!(press(&mut form, &model, key).is_none());
    }
    assert!(!form.modal_open);
    assert_eq!(form.pending, None);
    assert_eq!(form.field, Field::Password);
    assert_eq!(current_session(&model, &form).unwrap().id, "niri");

    assert!(press(&mut form, &model, KeyCode::F(11)).is_none());
    assert_eq!(form.pending, Some(Pending::Reboot));
    assert!(press(&mut form, &model, KeyCode::Enter).is_none());
    assert_eq!(form.pending, None);

    for key in [KeyCode::BackTab, KeyCode::Backspace, KeyCode::Char('a'), KeyCode::Tab] {
        assert!(press(&mut form, &model, key).is_none());
    }
    let Some(FormEvent::Submit(attempt)) = press(&mut form, &model, KeyCode::Enter) else {
        panic!("expected the selected login after leaving the picker and cancelling reboot");
    };
    assert_eq!(attempt.username, "alica");
    assert_eq!(attempt.session_id, "niri");
    assert_eq!(attempt.password.expose_secret(), "pässword");
    assert!(form.password.is_empty());

    let Some(FormEvent::Submit(retry)) = press(&mut form, &model, KeyCode::Enter) else {
        panic!("PAM must receive a fresh passwordless attempt");
    };
    assert_eq!(retry.username, attempt.username);
    assert_eq!(retry.session_id, attempt.session_id);
    assert!(retry.password.expose_secret().is_empty());
}

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

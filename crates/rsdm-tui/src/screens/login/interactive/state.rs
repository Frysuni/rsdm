use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rsdm_core::{
    domain::{PasswordSecret, Session},
    ports::{LoginAttempt, LoginUiEvent, LoginUiModel, MAX_PASSWORD_BYTES, MAX_USERNAME_BYTES},
};
use zeroize::Zeroizing;

pub(super) use rsdm_ui::{Field, Pending};

pub(super) enum FormEvent {
    Ui(LoginUiEvent),
    Submit(LoginAttempt),
}

pub(super) struct FormState {
    pub(super) username: String,
    pub(super) password: Zeroizing<String>,
    pub(super) field: Field,
    pub(super) selected: usize,
    pub(super) modal_open: bool,
    pub(super) pending: Option<Pending>,
}

impl FormState {
    pub(super) fn new(model: &LoginUiModel<'_>) -> Self {
        Self {
            username: model.remembered_username.unwrap_or_default().to_string(),
            password: Zeroizing::new(String::new()),
            field: if model.remembered_username.is_some() {
                Field::Password
            } else {
                Field::Username
            },
            selected: selected_index(model),
            modal_open: false,
            pending: None,
        }
    }

    fn arm(&mut self, action: Pending) -> Option<FormEvent> {
        self.pending = Some(action);
        None
    }

    fn submit(&mut self, model: &LoginUiModel<'_>) -> Option<FormEvent> {
        let session_id = current_session(model, self)?.id.clone();
        if self.username.trim().is_empty() {
            return None;
        }
        Some(FormEvent::Submit(LoginAttempt {
            username: self.username.trim().to_string(),
            password: PasswordSecret::new(std::mem::take(&mut *self.password)),
            session_id,
        }))
    }

    fn advance_or_submit(&mut self, model: &LoginUiModel<'_>) -> Option<FormEvent> {
        if self.username.trim().is_empty() {
            self.field = Field::Username;
            return None;
        }
        if self.password.is_empty() {
            self.field = Field::Password;
            return None;
        }
        if session_field_visible(model) && model.sessions.is_empty() {
            self.field = Field::Session;
            return None;
        }
        self.submit(model)
    }

    fn push_char(&mut self, ch: char) {
        match self.field {
            Field::Username if self.username.len() + ch.len_utf8() <= MAX_USERNAME_BYTES => {
                self.username.push(ch);
            }
            Field::Password if self.password.len() + ch.len_utf8() <= MAX_PASSWORD_BYTES => {
                self.password.push(ch);
            }
            _ => {}
        }
    }

    fn backspace(&mut self) {
        match self.field {
            Field::Username => {
                self.username.pop();
            }
            Field::Password => {
                self.password.pop();
            }
            Field::Session => {}
        }
    }

    fn next_field(&mut self, model: &LoginUiModel<'_>) {
        self.field = match (self.field, session_field_visible(model)) {
            (Field::Username, _) => Field::Password,
            (Field::Password, true) => Field::Session,
            (Field::Password, false) | (Field::Session, _) => Field::Username,
        };
    }

    fn previous_field(&mut self, model: &LoginUiModel<'_>) {
        self.field = match (self.field, session_field_visible(model)) {
            (Field::Username, true) => Field::Session,
            (Field::Username, false) | (Field::Session, _) => Field::Password,
            (Field::Password, _) => Field::Username,
        };
    }

    fn next_session(&mut self, model: &LoginUiModel<'_>) {
        if !model.sessions.is_empty() {
            self.selected = (self.selected + 1) % model.sessions.len();
        }
    }

    fn previous_session(&mut self, model: &LoginUiModel<'_>) {
        if !model.sessions.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(model.sessions.len() - 1);
        }
    }
}

pub(super) fn handle_key(
    key: KeyEvent,
    form: &mut FormState,
    model: &LoginUiModel<'_>,
) -> Option<FormEvent> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(FormEvent::Ui(LoginUiEvent::ExitToTty));
    }

    if form.modal_open {
        return handle_modal_key(key, form, model);
    }

    if let Some(armed) = form.pending.take() {
        if key.code == pending_key(armed) {
            return Some(pending_event(armed));
        }
        return None;
    }

    handle_form_key(key.code, form, model)
}

fn handle_form_key(
    key: KeyCode,
    form: &mut FormState,
    model: &LoginUiModel<'_>,
) -> Option<FormEvent> {
    match key {
        KeyCode::Esc => form.arm(Pending::Exit),
        KeyCode::F(11) => form.arm(Pending::Reboot),
        KeyCode::F(12) => form.arm(Pending::Shutdown),
        KeyCode::Tab | KeyCode::Down => {
            form.next_field(model);
            None
        }
        KeyCode::BackTab | KeyCode::Up => {
            form.previous_field(model);
            None
        }
        KeyCode::Left if form.field == Field::Session => {
            form.previous_session(model);
            None
        }
        KeyCode::Right if form.field == Field::Session => {
            form.next_session(model);
            None
        }
        KeyCode::F(2) if session_picker_enabled(model) => {
            form.modal_open = true;
            None
        }
        KeyCode::Enter if form.field == Field::Session && session_picker_enabled(model) => {
            form.modal_open = true;
            None
        }
        KeyCode::Enter => form.advance_or_submit(model),
        KeyCode::Backspace => {
            form.backspace();
            None
        }
        KeyCode::Char(ch) => {
            form.push_char(ch);
            None
        }
        _ => None,
    }
}

fn pending_key(pending: Pending) -> KeyCode {
    match pending {
        Pending::Exit => KeyCode::Esc,
        Pending::Reboot => KeyCode::F(11),
        Pending::Shutdown => KeyCode::F(12),
    }
}

fn pending_event(pending: Pending) -> FormEvent {
    FormEvent::Ui(match pending {
        Pending::Exit => LoginUiEvent::ExitToTty,
        Pending::Reboot => LoginUiEvent::Reboot,
        Pending::Shutdown => LoginUiEvent::Shutdown,
    })
}

fn handle_modal_key(
    key: KeyEvent,
    form: &mut FormState,
    model: &LoginUiModel<'_>,
) -> Option<FormEvent> {
    match key.code {
        KeyCode::Esc => form.modal_open = false,
        KeyCode::Up => form.previous_session(model),
        KeyCode::Down => form.next_session(model),
        KeyCode::Enter => form.modal_open = false,
        _ => {}
    }
    None
}

pub(super) fn current_session<'a>(
    model: &'a LoginUiModel<'_>,
    form: &FormState,
) -> Option<&'a Session> {
    model.sessions.get(form.selected)
}

pub(super) fn session_field_visible(model: &LoginUiModel<'_>) -> bool {
    model.config.dm.uses_picker()
}

pub(super) fn session_picker_enabled(model: &LoginUiModel<'_>) -> bool {
    session_field_visible(model) && model.sessions.len() > 1
}

fn selected_index(model: &LoginUiModel<'_>) -> usize {
    model
        .remembered_session
        .and_then(|id| model.sessions.iter().position(|session| session.id == id))
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;

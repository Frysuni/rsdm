mod state;
mod terminal;

use std::{io, sync::atomic::Ordering, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{Terminal, backend::CrosstermBackend};
use rsdm_core::{
    domain::PasswordRendering,
    ports::{LoginAttempt, LoginAttemptOutcome, LoginUi, LoginUiEvent, LoginUiModel, UiError},
};
use rsdm_ui::{Design, LoginScene, Menu, MenuKey, Surface, banner};

use crate::surface::TuiSurface;

use self::{
    state::{FormEvent, FormState, handle_key},
    terminal::{TerminalGuard, install_panic_hook, open_tty},
};

struct Status {
    text: String,
    is_error: bool,
}

impl Status {
    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }

    fn progress(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }
}

fn build_scene<'a>(
    model: &'a LoginUiModel<'a>,
    form: &'a FormState,
    design: &Design,
    status: Option<&'a Status>,
) -> LoginScene<'a> {
    let password_preview = match design.password_mode {
        PasswordRendering::Hidden => String::new(),
        PasswordRendering::Asterisks => "*".repeat(form.password.chars().count()),
    };
    let session_name = model.sessions.get(form.selected).map(|s| s.name.as_str());
    let (message, message_is_error) = match status {
        Some(status) => (Some(status.text.as_str()), status.is_error),
        None => (model.error_message, true),
    };
    LoginScene {
        title: banner::title_lines(design, session_name),
        hostname: design.show_hostname.then(banner::hostname),
        clock: design.show_clock.then(banner::clock_text),
        username: &form.username,
        password_preview,
        field: form.field,
        pending: form.pending,
        message,
        message_is_error,
        sessions: model.sessions,
        selected: form.selected,
        session_field_visible: state::session_field_visible(model),
        session_picker_enabled: state::session_picker_enabled(model),
        picker_open: form.modal_open,
    }
}

const TICK: Duration = Duration::from_secs(1);

const ANIMATION_TICK: Duration = Duration::from_millis(66);

#[derive(Debug, Default)]
pub struct RatatuiLoginUi;

impl LoginUi for RatatuiLoginUi {
    fn run(
        &mut self,
        model: LoginUiModel<'_>,
        attempt: &mut dyn FnMut(LoginAttempt) -> LoginAttemptOutcome,
    ) -> Result<LoginUiEvent, UiError> {
        run_prompt(model, attempt).map_err(|error| UiError::Terminal(error.to_string()))
    }
}

fn menu_key(key: KeyEvent) -> Option<MenuKey> {
    match key.code {
        KeyCode::Up => Some(MenuKey::Up),
        KeyCode::Down => Some(MenuKey::Down),
        KeyCode::Left => Some(MenuKey::Left),
        KeyCode::Right => Some(MenuKey::Right),
        KeyCode::Enter => Some(MenuKey::Enter),
        KeyCode::Esc => Some(MenuKey::Esc),
        _ => None,
    }
}

fn run_prompt(
    model: LoginUiModel<'_>,
    attempt: &mut dyn FnMut(LoginAttempt) -> LoginAttemptOutcome,
) -> Result<LoginUiEvent, io::Error> {
    install_panic_hook();
    let tty = open_tty(&model.config.dm.tty.path)?;
    let backend = CrosstermBackend::new(tty.try_clone()?);
    let _guard = TerminalGuard::enter(&model.config.dm.tty.path, tty)?;
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let mut form = FormState::new(&model);
    let mut design = Design::from_config(&model.config.dm.design);
    let mut menu = Menu::new();
    let menu_enabled = model.config.dm.design.menu;
    let mut animation_frame: u64 = 0;
    let mut status: Option<Status> = None;

    loop {
        if model.terminate.load(Ordering::SeqCst) {
            return Ok(LoginUiEvent::Terminated);
        }
        let size = terminal.size()?;
        if size.width < rsdm_ui::screen::MIN_W || size.height < rsdm_ui::screen::MIN_H {
            return Err(io::Error::other("terminal too small for the greeter"));
        }
        terminal.draw(|frame| {
            let mut surface = TuiSurface::new(frame.buffer_mut());
            surface.clear(design.palette().bg_base);
            let scene = build_scene(&model, &form, &design, status.as_ref());
            rsdm_ui::render_login(&mut surface, &design, &scene, &menu, animation_frame);
        })?;
        animation_frame = animation_frame.wrapping_add(1);

        let tick = if design.background.is_animated() {
            ANIMATION_TICK
        } else {
            TICK
        };
        // A SIGTERM interrupts the poll (the stop handler is installed without
        // SA_RESTART); loop back to the terminate check instead of failing.
        match event::poll(tick) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if menu.is_open() {
                    if let Some(mk) = menu_key(key) {
                        menu.handle_key(mk, &mut design);
                    }
                    continue;
                }
                if menu_enabled && key.code == KeyCode::F(1) {
                    form.pending = None;
                    menu.open();
                    continue;
                }
                match handle_key(key, &mut form, &model) {
                    Some(FormEvent::Ui(event)) => return Ok(event),
                    Some(FormEvent::Submit(submitted)) => {
                        status = Some(Status::progress("Authenticating..."));
                        terminal.draw(|frame| {
                            let mut surface = TuiSurface::new(frame.buffer_mut());
                            surface.clear(design.palette().bg_base);
                            let scene = build_scene(&model, &form, &design, status.as_ref());
                            rsdm_ui::render_login(
                                &mut surface,
                                &design,
                                &scene,
                                &menu,
                                animation_frame,
                            );
                        })?;
                        match attempt(submitted) {
                            LoginAttemptOutcome::Failure(message) => {
                                status = Some(Status::error(message));
                                form.field = state::Field::Password;
                            }
                            LoginAttemptOutcome::SessionReady => {
                                return Ok(LoginUiEvent::SessionReady);
                            }
                        }
                    }
                    None => {}
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FormState, build_scene};
    use crate::surface::TuiSurface;
    use ratatui::{Terminal, backend::TestBackend};
    use rsdm_core::{
        domain::{AppConfig, Session},
        ports::LoginUiModel,
    };
    use rsdm_ui::{Design, Menu, Surface};

    fn sessions() -> Vec<Session> {
        vec![Session::new("niri", "Niri", "niri-session", "/x")]
    }

    fn render_at(width: u16, height: u16) -> String {
        static TERMINATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
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
        let form = FormState::new(&model);
        let design = Design::from_config(&config.dm.design);
        let menu = Menu::new();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let mut surface = TuiSurface::new(frame.buffer_mut());
                surface.clear(design.palette().bg_base);
                let scene = build_scene(&model, &form, &design, None);
                rsdm_ui::render_login(&mut surface, &design, &scene, &menu, 0);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn renders_login_form() {
        let text = render_at(100, 32);
        assert!(text.contains("LOGIN"), "expected the framed caption");
        assert!(
            text.contains('\u{2588}'),
            "expected the generated block-art title"
        );
        assert!(text.contains("User"), "expected a user field");
        assert!(text.contains("Password"), "expected a password field");
        assert!(text.contains("F11"), "expected a reboot action");
        assert!(text.contains("F12"), "expected a shutdown action");
        assert!(text.contains("Menu"), "expected the F1 menu hint");
    }

    #[test]
    fn small_terminal_shows_a_notice_without_panicking() {
        let text = render_at(20, 6);
        assert!(text.contains("too small"));
    }
}

use std::process::Command;

use rsdm_ui::{LockPending, MenuKey, MenuOutcome};
use smithay_client_toolkit::{
    reexports::client::{
        Connection, QueueHandle,
        protocol::{wl_keyboard, wl_surface},
    },
    seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
};

use super::App;
use crate::auth::{Attempt, Authenticator};

impl App {
    pub(super) fn unlock(&mut self) {
        if let Some(lock) = self.session_lock.take() {
            lock.unlock();
            self.unlocked = true;
        }
        self.exit = true;
    }

    fn submit(&mut self) {
        if self.model.is_empty() {
            return;
        }
        let password = self.model.take_password();
        let authenticator = Authenticator {
            verifier: &self.ctx.verifier,
            limiter: &self.ctx.limiter,
            username: &self.ctx.username,
            pam_service: &self.ctx.pam_service,
        };
        match authenticator.attempt(&password) {
            Attempt::Unlocked => self.unlock(),
            Attempt::Failed(message) | Attempt::RateLimited(message) => {
                self.model.set_error(message);
            }
        }
    }

    fn handle_menu_key(&mut self, keysym: Keysym) {
        let Some(key) = menu_key(keysym) else {
            self.needs_redraw = true;
            return;
        };
        let previous_policy = self
            .menu
            .lock_settings()
            .map(|settings| settings.secondary_output);
        let outcome = self.menu.handle_key(key, &mut self.ctx.design);
        let current = self.menu.lock_settings();
        if previous_policy != current.map(|settings| settings.secondary_output)
            && let Some(settings) = current
        {
            self.apply_secondary_output_policy(settings.secondary_output);
        }
        if outcome == MenuOutcome::SaveRequested {
            self.save_runtime_settings();
        }
        self.needs_redraw = true;
    }

    fn save_runtime_settings(&mut self) {
        let Some(settings) = self.menu.lock_settings() else {
            return;
        };
        let mut design = self.ctx.design_config.clone();
        self.ctx.design.apply_runtime_to_config(&mut design);
        let result = rsdm_infra::config::save_lock_runtime_settings(
            &self.ctx.config_path,
            &design,
            settings.size,
            settings.secondary_output,
        );
        match result {
            Ok(()) => {
                self.ctx.design_config = design;
                self.menu.set_notice(
                    format!("Saved to {}", self.ctx.config_path.display()),
                    false,
                );
            }
            Err(error) => {
                tracing::error!(%error, "could not save runtime lock settings");
                self.menu.set_notice(format!("Save failed: {error}"), true);
            }
        }
        self.needs_redraw = true;
    }

    fn handle_regular_key(&mut self, event: KeyEvent) {
        match event.keysym {
            Keysym::Return | Keysym::KP_Enter => self.submit(),
            Keysym::BackSpace => self.model.backspace(),
            Keysym::Escape => self.model.clear_password(),
            Keysym::F11 => self.pending = Some(LockPending::Reboot),
            Keysym::F12 => self.pending = Some(LockPending::Shutdown),
            Keysym::F9 if self.ctx.hibernate_available => {
                self.pending = Some(LockPending::Hibernate);
            }
            Keysym::F10 => self.pending = Some(LockPending::Sleep),
            _ => {
                if let Some(text) = event.utf8 {
                    for character in text.chars() {
                        self.model.push(character);
                    }
                }
            }
        }
        self.needs_redraw = true;
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if self.menu.is_open() {
            self.handle_menu_key(event.keysym);
            return;
        }
        if let Some(armed) = self.pending.take() {
            if event.keysym == pending_keysym(armed)
                && let Some(message) = run_power_action(armed)
            {
                self.model.set_error(message);
            }
            self.needs_redraw = true;
            return;
        }
        if self.menu_enabled && event.keysym == Keysym::F1 {
            self.menu.open();
            self.needs_redraw = true;
            return;
        }
        self.handle_regular_key(event);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: u32,
    ) {
    }
}

pub(super) fn hibernate_available() -> bool {
    std::fs::read_to_string("/sys/power/state")
        .map(|states| states.split_whitespace().any(|state| state == "disk"))
        .unwrap_or(false)
}

fn menu_key(keysym: Keysym) -> Option<MenuKey> {
    match keysym {
        Keysym::Up => Some(MenuKey::Up),
        Keysym::Down => Some(MenuKey::Down),
        Keysym::Left => Some(MenuKey::Left),
        Keysym::Right => Some(MenuKey::Right),
        Keysym::Return | Keysym::KP_Enter => Some(MenuKey::Enter),
        Keysym::Escape => Some(MenuKey::Esc),
        _ => None,
    }
}

fn pending_keysym(action: LockPending) -> Keysym {
    match action {
        LockPending::Reboot => Keysym::F11,
        LockPending::Shutdown => Keysym::F12,
        LockPending::Hibernate => Keysym::F9,
        LockPending::Sleep => Keysym::F10,
    }
}

fn run_power_action(action: LockPending) -> Option<String> {
    let verb = match action {
        LockPending::Reboot => "reboot",
        LockPending::Shutdown => "poweroff",
        LockPending::Hibernate => "hibernate",
        LockPending::Sleep => "suspend",
    };
    match Command::new("systemctl").arg(verb).status() {
        Ok(status) if status.success() => None,
        Ok(status) => {
            tracing::error!(%status, action = verb, "power action rejected");
            Some(format!("{verb} was rejected: {status}"))
        }
        Err(error) => {
            tracing::error!(%error, action = verb, "power action failed");
            Some(format!("{verb} failed: {error}"))
        }
    }
}

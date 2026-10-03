use rsdm_core::domain::{MAX_LOCK_SIZE, SecondaryOutput};

use crate::design::{self, Design};

mod render;
mod view;

pub use view::{MenuRow, MenuView};

const ROOT_ROWS: usize = 7;
const WALLPAPER_ROOT_ROWS: usize = 2;
const LOCK_ROOT_ROWS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuMode {
    Root,
    Themes,
    Borders,
    Backgrounds,
    TitleFonts,
    Speed,
    Dim,
    BackgroundOpacity,
    LockSize,
    SecondaryOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuOutcome {
    Ignored,
    Redraw,
    Closed,
    SaveRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockMenuSettings {
    pub size: Option<u8>,
    pub secondary_output: SecondaryOutput,
}

#[derive(Debug)]
pub struct Menu {
    open: bool,
    mode: MenuMode,
    index: usize,
    wallpaper_controls: bool,
    hints_enabled: bool,
    lock_settings: Option<LockMenuSettings>,
    notice: Option<(String, bool)>,
}

impl Default for Menu {
    fn default() -> Self {
        Self::new()
    }
}

impl Menu {
    pub fn new() -> Self {
        Self {
            open: false,
            mode: MenuMode::Root,
            index: 0,
            wallpaper_controls: false,
            hints_enabled: true,
            lock_settings: None,
            notice: None,
        }
    }

    pub fn show_wallpaper_controls(&mut self, enabled: bool) {
        self.wallpaper_controls = enabled;
    }

    pub fn enable_lock_controls(&mut self, settings: LockMenuSettings) {
        self.lock_settings = Some(settings);
    }

    pub fn lock_settings(&self) -> Option<LockMenuSettings> {
        self.lock_settings
    }

    pub fn hints_enabled(&self) -> bool {
        self.hints_enabled
    }

    pub fn set_notice(&mut self, message: impl Into<String>, error: bool) {
        self.notice = Some((message.into(), error));
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
        self.go_root();
    }

    pub fn handle_key(&mut self, key: MenuKey, design: &mut Design) -> MenuOutcome {
        if !self.open {
            return MenuOutcome::Ignored;
        }
        if key != MenuKey::Enter {
            self.notice = None;
        }

        match key {
            MenuKey::Up | MenuKey::Left => self.move_selection(-1, design),
            MenuKey::Down | MenuKey::Right => self.move_selection(1, design),
            MenuKey::Enter => self.select(design),
            MenuKey::Esc if self.mode == MenuMode::Root => {
                self.close();
                MenuOutcome::Closed
            }
            MenuKey::Esc => {
                self.go_root();
                MenuOutcome::Redraw
            }
        }
    }

    fn move_selection(&mut self, direction: isize, design: &mut Design) -> MenuOutcome {
        let len = self.len();
        self.index = if direction < 0 {
            (self.index + len - 1) % len
        } else {
            (self.index + 1) % len
        };
        self.apply_highlight(design);
        MenuOutcome::Redraw
    }

    fn select(&mut self, design: &Design) -> MenuOutcome {
        if self.mode != MenuMode::Root {
            return self.finish_submenu();
        }

        match self.index {
            0 => {
                self.close();
                MenuOutcome::Closed
            }
            1 => self.open_submenu(MenuMode::Themes, theme_index(design)),
            2 => self.open_submenu(MenuMode::Borders, border_index(design)),
            3 => self.open_submenu(MenuMode::Backgrounds, background_index(design)),
            4 => self.open_submenu(MenuMode::TitleFonts, title_font_index(design)),
            5 => self.open_submenu(MenuMode::Speed, design.background_speed as usize),
            6 => {
                self.hints_enabled = !self.hints_enabled;
                MenuOutcome::Redraw
            }
            7 if self.wallpaper_controls => self.open_submenu(MenuMode::Dim, design.dim as usize),
            8 if self.wallpaper_controls => self.open_submenu(
                MenuMode::BackgroundOpacity,
                design.background_opacity as usize,
            ),
            index if self.lock_settings.is_some() => self.select_lock_row(index),
            _ => MenuOutcome::Redraw,
        }
    }

    fn finish_submenu(&mut self) -> MenuOutcome {
        if self.index == 0 {
            self.go_root();
            return MenuOutcome::Redraw;
        }
        self.close();
        MenuOutcome::Closed
    }

    fn select_lock_row(&mut self, index: usize) -> MenuOutcome {
        let lock_row = index.saturating_sub(self.lock_rows_start());
        match lock_row {
            0 => {
                let active = self
                    .lock_settings
                    .and_then(|settings| settings.size)
                    .map_or(0, usize::from);
                self.open_submenu(MenuMode::LockSize, active)
            }
            1 => {
                let active = self
                    .lock_settings
                    .and_then(|settings| {
                        SecondaryOutput::ALL
                            .iter()
                            .position(|policy| *policy == settings.secondary_output)
                    })
                    .unwrap_or(0);
                self.open_submenu(MenuMode::SecondaryOutput, active)
            }
            _ => MenuOutcome::SaveRequested,
        }
    }

    fn apply_highlight(&mut self, design: &mut Design) {
        let Some(roster_index) = self.index.checked_sub(1) else {
            return;
        };

        match self.mode {
            MenuMode::Root => {}
            MenuMode::Themes => {
                if let Some(theme) = design::THEMES.get(roster_index) {
                    design.theme = *theme;
                }
            }
            MenuMode::Borders => {
                if let Some(border) = design::BORDERS.get(roster_index) {
                    design.border = *border;
                }
            }
            MenuMode::Backgrounds => {
                if let Some(background) = design::BACKGROUNDS.get(roster_index) {
                    design.background = *background;
                }
            }
            MenuMode::TitleFonts => {
                if let Some(font) = design::title_fonts().get(roster_index) {
                    design.title_font.clone_from(font);
                }
            }
            MenuMode::Speed => {
                design.background_speed = (roster_index as u8).min(design::MAX_SPEED);
            }
            MenuMode::Dim => {
                design.dim = (roster_index as u8).min(design::MAX_DIM);
            }
            MenuMode::BackgroundOpacity => {
                design.background_opacity = (roster_index as u8).min(design::MAX_OPACITY);
            }
            MenuMode::LockSize => self.apply_lock_size(),
            MenuMode::SecondaryOutput => self.apply_secondary_output(roster_index),
        }
    }

    fn apply_lock_size(&mut self) {
        let Some(settings) = self.lock_settings.as_mut() else {
            return;
        };
        settings.size = match self.index {
            1 => None,
            index => Some(((index - 1) as u8).min(MAX_LOCK_SIZE)),
        };
    }

    fn apply_secondary_output(&mut self, roster_index: usize) {
        let Some(policy) = SecondaryOutput::ALL.get(roster_index) else {
            return;
        };
        if let Some(settings) = self.lock_settings.as_mut() {
            settings.secondary_output = *policy;
        }
    }

    fn len(&self) -> usize {
        match self.mode {
            MenuMode::Root => self.root_rows(),
            MenuMode::Themes => design::THEMES.len() + 1,
            MenuMode::Borders => design::BORDERS.len() + 1,
            MenuMode::Backgrounds => design::BACKGROUNDS.len() + 1,
            MenuMode::TitleFonts => design::title_fonts().len() + 1,
            MenuMode::Speed => design::MAX_SPEED as usize + 2,
            MenuMode::Dim => design::MAX_DIM as usize + 2,
            MenuMode::BackgroundOpacity => design::MAX_OPACITY as usize + 2,
            MenuMode::LockSize => MAX_LOCK_SIZE as usize + 2,
            MenuMode::SecondaryOutput => SecondaryOutput::ALL.len() + 1,
        }
    }

    fn root_rows(&self) -> usize {
        ROOT_ROWS
            + usize::from(self.wallpaper_controls) * WALLPAPER_ROOT_ROWS
            + usize::from(self.lock_settings.is_some()) * LOCK_ROOT_ROWS
    }

    fn lock_rows_start(&self) -> usize {
        ROOT_ROWS + usize::from(self.wallpaper_controls) * WALLPAPER_ROOT_ROWS
    }

    fn open_submenu(&mut self, mode: MenuMode, active: usize) -> MenuOutcome {
        self.mode = mode;
        self.index = active + 1;
        MenuOutcome::Redraw
    }

    fn close(&mut self) {
        self.open = false;
        self.go_root();
    }

    fn go_root(&mut self) {
        self.mode = MenuMode::Root;
        self.index = 0;
    }
}

fn theme_index(design: &Design) -> usize {
    design::THEMES
        .iter()
        .position(|theme| *theme == design.theme)
        .unwrap_or(0)
}

fn border_index(design: &Design) -> usize {
    design::BORDERS
        .iter()
        .position(|border| *border == design.border)
        .unwrap_or(0)
}

fn background_index(design: &Design) -> usize {
    design::BACKGROUNDS
        .iter()
        .position(|background| *background == design.background)
        .unwrap_or(0)
}

fn title_font_index(design: &Design) -> usize {
    design::title_fonts()
        .iter()
        .position(|font| font == &design.title_font)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;

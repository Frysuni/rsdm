use rsdm_core::domain::{MAX_LOCK_SIZE, SecondaryOutput};

use super::{Menu, MenuMode};
use crate::design::{self, Design};

#[derive(Debug, Clone)]
pub struct MenuRow {
    pub label: String,
    pub selected: bool,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct MenuView {
    pub title: String,
    pub rows: Vec<MenuRow>,
}

impl Menu {
    pub fn view(&self, design: &Design) -> MenuView {
        let (title, rows) = match self.mode {
            MenuMode::Root => ("Design", self.root_rows_view()),
            MenuMode::Themes => (
                "Theme",
                self.roster_rows(design::THEMES, design::theme_label, design.theme),
            ),
            MenuMode::Borders => (
                "Border",
                self.roster_rows(design::BORDERS, design::border_label, design.border),
            ),
            MenuMode::Backgrounds => (
                "Background",
                self.roster_rows(
                    design::BACKGROUNDS,
                    design::background_label,
                    design.background,
                ),
            ),
            MenuMode::TitleFonts => ("Title font", self.font_rows(&design.title_font)),
            MenuMode::Speed => ("Speed", self.speed_rows(design.background_speed)),
            MenuMode::Dim => (
                "Wallpaper dim",
                self.level_rows(design.dim, design::MAX_DIM, "0 (black)", "full"),
            ),
            MenuMode::BackgroundOpacity => (
                "Background opacity",
                self.level_rows(
                    design.background_opacity,
                    design::MAX_OPACITY,
                    "0 (off)",
                    "opaque",
                ),
            ),
            MenuMode::LockSize => ("Lock size", self.lock_size_rows()),
            MenuMode::SecondaryOutput => ("Secondary outputs", self.secondary_output_rows()),
        };

        MenuView {
            title: title.to_string(),
            rows,
        }
    }

    fn root_rows_view(&self) -> Vec<MenuRow> {
        let mut rows = vec![
            row("Close menu", self.index == 0, false),
            row("Themes >", self.index == 1, false),
            row("Borders >", self.index == 2, false),
            row("Backgrounds >", self.index == 3, false),
            row("Title font >", self.index == 4, false),
            row("Speed >", self.index == 5, false),
        ];

        if self.wallpaper_controls {
            rows.push(row("Wallpaper dim >", self.index == 6, false));
            rows.push(row("Background opacity >", self.index == 7, false));
        }
        if let Some(settings) = self.lock_settings {
            let size = settings
                .size
                .map_or_else(|| "auto".to_string(), |size| size.to_string());
            rows.push(row(
                &format!("Lock size: {size} >"),
                self.index == rows.len(),
                false,
            ));
            rows.push(row(
                &format!(
                    "Secondary outputs: {} >",
                    settings.secondary_output.as_str()
                ),
                self.index == rows.len(),
                false,
            ));
            rows.push(row("Save settings", self.index == rows.len(), false));
        }

        rows
    }

    fn roster_rows<T: Copy + PartialEq>(
        &self,
        roster: &[T],
        label: fn(T) -> &'static str,
        active: T,
    ) -> Vec<MenuRow> {
        let mut rows = vec![row("< Back", self.index == 0, false)];
        for (index, &item) in roster.iter().enumerate() {
            rows.push(row(label(item), self.index == index + 1, item == active));
        }
        rows
    }

    fn speed_rows(&self, active: u8) -> Vec<MenuRow> {
        let mut rows = vec![row("< Back", self.index == 0, false)];
        for value in 0..=design::MAX_SPEED {
            let label = match value {
                0 => "0 (frozen)".to_string(),
                design::DEFAULT_SPEED => format!("{value} (default)"),
                _ => value.to_string(),
            };
            rows.push(row(
                &label,
                self.index == value as usize + 1,
                value == active,
            ));
        }
        rows
    }

    fn level_rows(&self, active: u8, max: u8, zero: &str, top: &str) -> Vec<MenuRow> {
        let mut rows = vec![row("< Back", self.index == 0, false)];
        for value in 0..=max {
            let label = match value {
                0 => zero.to_string(),
                value if value == max => format!("{value} ({top})"),
                _ => value.to_string(),
            };
            rows.push(row(
                &label,
                self.index == value as usize + 1,
                value == active,
            ));
        }
        rows
    }

    fn font_rows(&self, active: &str) -> Vec<MenuRow> {
        let mut rows = vec![row("< Back", self.index == 0, false)];
        for (index, font) in design::title_fonts().iter().enumerate() {
            rows.push(row(font, self.index == index + 1, font == active));
        }
        rows
    }

    fn lock_size_rows(&self) -> Vec<MenuRow> {
        let active = self.lock_settings.and_then(|settings| settings.size);
        let mut rows = vec![
            row("< Back", self.index == 0, false),
            row("Auto (TTY-like)", self.index == 1, active.is_none()),
        ];
        for size in 1..=MAX_LOCK_SIZE {
            rows.push(row(
                &size.to_string(),
                self.index == size as usize + 1,
                active == Some(size),
            ));
        }
        rows
    }

    fn secondary_output_rows(&self) -> Vec<MenuRow> {
        let active = self
            .lock_settings
            .map(|settings| settings.secondary_output)
            .unwrap_or_default();
        let mut rows = vec![row("< Back", self.index == 0, false)];
        for (index, policy) in SecondaryOutput::ALL.iter().enumerate() {
            rows.push(row(
                policy.as_str(),
                self.index == index + 1,
                *policy == active,
            ));
        }
        rows
    }
}

fn row(label: &str, selected: bool, active: bool) -> MenuRow {
    MenuRow {
        label: label.to_string(),
        selected,
        active,
    }
}

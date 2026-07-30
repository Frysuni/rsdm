//! The live design state, shared by both fronts.
//!
//! [`Design`] is always built from the config at startup
//! ([`Design::from_config`]); the runtime switcher ([`crate::menu`]) mutates it
//! in place. The lock front can explicitly copy those live fields back into its
//! config when the user selects Save settings; otherwise the next launch starts
//! from the original config.

use std::sync::OnceLock;

use figrs::Figlet;
use rsdm_core::domain::{
    Background, BorderStyle, DEFAULT_TITLE_FONT, DesignConfig, LogoPreset, Palette,
    PasswordRendering, ThemePreset, TitleSource, theme,
};

/// The natural background animation rate; `background_speed` scales around it.
pub const DEFAULT_SPEED: u8 = 5;
/// The largest accepted background speed.
pub const MAX_SPEED: u8 = 10;
/// The largest wallpaper-dim level. `0` blacks the wallpaper out, `MAX_DIM`
/// leaves it untouched.
pub const MAX_DIM: u8 = 10;
/// The largest animated-background-opacity level. `0` shows only the wallpaper,
/// `MAX_OPACITY` draws the animation fully opaque.
pub const MAX_OPACITY: u8 = 10;

/// The currently active look. The palette is derived from [`Self::theme`] on
/// demand so the two fronts never disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Design {
    pub theme: ThemePreset,
    pub border: BorderStyle,
    pub background: Background,
    /// Background animation speed, `0` (frozen) to [`MAX_SPEED`].
    pub background_speed: u8,
    /// Locker-only wallpaper dim level, `0` (wallpaper fully blacked out) to
    /// [`MAX_DIM`] (wallpaper untouched). The greeter has no wallpaper and
    /// ignores it. Set from `lock.wallpaper_dim`; switchable at runtime. Convert
    /// to a darkening alpha with [`Design::dim_alpha`].
    pub dim: u8,
    /// Locker-only opacity level of the animated background where it is drawn
    /// over a wallpaper, `0` (wallpaper only) to [`MAX_OPACITY`] (opaque). Set
    /// from `lock.background_opacity`; switchable at runtime. Convert to a
    /// `0..=255` opacity with [`Design::background_opacity_byte`].
    pub background_opacity: u8,
    pub password_mode: PasswordRendering,
    pub show_clock: bool,
    pub show_hostname: bool,
    // Title resolution. `font` is runtime-switchable; the source/text/preset
    // come from config and are carried here so the banner is built from
    // `Design` alone.
    pub title_source: TitleSource,
    pub title_text: Option<String>,
    pub title_preset: LogoPreset,
    pub title_font: String,
}

impl Design {
    /// Build the starting design from config. This is the only constructor, so
    /// every run begins from the config.
    pub fn from_config(d: &DesignConfig) -> Self {
        Self {
            theme: d.theme,
            border: d.border_style,
            background: d.background,
            background_speed: d.background_speed.min(MAX_SPEED),
            // The wallpaper knobs are locker-only; the greeter ignores them.
            dim: d.wallpaper_dim.min(MAX_DIM),
            background_opacity: d.background_opacity.min(MAX_OPACITY),
            password_mode: d.password_mode,
            show_clock: d.show_clock,
            show_hostname: d.show_hostname,
            title_source: d.title_mode,
            title_text: d.title_text.clone(),
            title_preset: d.title_preset,
            title_font: normalize_title_font(&d.title_font),
        }
    }

    /// Copy only runtime-editable fields back into a config design. Settings
    /// not exposed by the menu remain untouched.
    pub fn apply_runtime_to_config(&self, target: &mut DesignConfig) {
        target.theme = self.theme;
        target.border_style = self.border;
        target.background = self.background;
        target.background_speed = self.background_speed;
        target.title_font.clone_from(&self.title_font);
        target.wallpaper_dim = self.dim;
        target.background_opacity = self.background_opacity;
    }

    /// The truecolor palette for the active theme.
    pub fn palette(&self) -> Palette {
        theme::palette(self.theme)
    }

    /// Darkening alpha (`0..=255`) the wallpaper is blended toward black by. Dim
    /// level `0` returns `255` (fully black), [`MAX_DIM`] returns `0` (the
    /// wallpaper untouched).
    pub fn dim_alpha(&self) -> u8 {
        let level = self.dim.min(MAX_DIM) as u32;
        ((255 * (MAX_DIM as u32 - level)) / MAX_DIM as u32) as u8
    }

    /// Opacity (`0..=255`) of the animated background over the wallpaper.
    /// Opacity level `0` returns `0` (wallpaper only), [`MAX_OPACITY`] returns
    /// `255` (fully opaque).
    pub fn background_opacity_byte(&self) -> u8 {
        let level = self.background_opacity.min(MAX_OPACITY) as u32;
        ((255 * level) / MAX_OPACITY as u32) as u8
    }
}

/// The full theme roster, in menu order. Mirrors the presets shipped by
/// sysc-greet.
pub const THEMES: &[ThemePreset] = &[
    ThemePreset::Minimal,
    ThemePreset::Monochrome,
    ThemePreset::Cyberpunk,
    ThemePreset::Catppuccin,
    ThemePreset::Gruvbox,
    ThemePreset::Nord,
    ThemePreset::Dracula,
    ThemePreset::TokyoNight,
    ThemePreset::Material,
    ThemePreset::Solarized,
    ThemePreset::Eldritch,
    ThemePreset::Rama,
    ThemePreset::Dark,
    ThemePreset::TransIsHardJob,
];

/// The full border roster, in menu order.
pub const BORDERS: &[BorderStyle] = &[
    BorderStyle::Classic,
    BorderStyle::Modern,
    BorderStyle::Minimal,
    BorderStyle::Ascii1,
    BorderStyle::Ascii2,
    BorderStyle::Ascii3,
    BorderStyle::Ascii4,
    BorderStyle::Wave,
    BorderStyle::Pulse,
];

/// The full background roster, in menu order.
pub const BACKGROUNDS: &[Background] = &[
    Background::None,
    Background::Matrix,
    Background::Fire,
    Background::Rain,
    Background::Plasma,
    Background::Starfield,
];

/// The full figrs font roster, in menu order.
pub fn title_fonts() -> &'static [String] {
    static FONTS: OnceLock<Vec<String>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut fonts = Figlet::fonts();
        fonts.sort_by_key(|font| font.to_lowercase());
        if !fonts
            .iter()
            .any(|font| font.eq_ignore_ascii_case(DEFAULT_TITLE_FONT))
        {
            fonts.insert(0, DEFAULT_TITLE_FONT.to_string());
        }
        fonts
    })
}

/// Resolve a configured font name case-insensitively.
pub fn normalize_title_font(name: &str) -> String {
    let candidate = name.trim();
    title_fonts()
        .iter()
        .find(|font| font.eq_ignore_ascii_case(candidate))
        .cloned()
        .unwrap_or_else(|| DEFAULT_TITLE_FONT.to_string())
}

/// Human-readable label for a theme, for the menu.
pub fn theme_label(theme: ThemePreset) -> &'static str {
    match theme {
        ThemePreset::Minimal => "Minimal",
        ThemePreset::Monochrome => "Monochrome",
        ThemePreset::Cyberpunk => "Cyberpunk",
        ThemePreset::Catppuccin => "Catppuccin",
        ThemePreset::Gruvbox => "Gruvbox",
        ThemePreset::Nord => "Nord",
        ThemePreset::Dracula => "Dracula",
        ThemePreset::TokyoNight => "Tokyo Night",
        ThemePreset::Material => "Material",
        ThemePreset::Solarized => "Solarized",
        ThemePreset::Eldritch => "Eldritch",
        ThemePreset::Rama => "Rama",
        ThemePreset::Dark => "Dark",
        ThemePreset::TransIsHardJob => "Trans Is Hard Job",
    }
}

/// Human-readable label for a border style, for the menu.
pub fn border_label(border: BorderStyle) -> &'static str {
    match border {
        BorderStyle::Classic => "Classic",
        BorderStyle::Modern => "Modern",
        BorderStyle::Minimal => "Minimal",
        BorderStyle::Ascii1 => "ASCII 1 (block)",
        BorderStyle::Ascii2 => "ASCII 2 (gradient)",
        BorderStyle::Ascii3 => "ASCII 3 (panel)",
        BorderStyle::Ascii4 => "ASCII 4 (banner)",
        BorderStyle::Wave => "Wave",
        BorderStyle::Pulse => "Pulse",
    }
}

/// Human-readable label for a background, for the menu.
pub fn background_label(background: Background) -> &'static str {
    match background {
        Background::None => "None",
        Background::Matrix => "Matrix",
        Background::Fire => "Fire",
        Background::Rain => "Rain",
        Background::Plasma => "Plasma",
        Background::Starfield => "Starfield",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_mirrors_ui() {
        let ui = DesignConfig {
            theme: ThemePreset::Gruvbox,
            background: Background::Matrix,
            ..Default::default()
        };
        let design = Design::from_config(&ui);
        assert_eq!(design.theme, ThemePreset::Gruvbox);
        assert_eq!(design.background, Background::Matrix);
        assert_eq!(design.palette(), theme::palette(ThemePreset::Gruvbox));
    }

    #[test]
    fn speed_is_clamped_from_config() {
        let ui = DesignConfig {
            background_speed: 200,
            ..Default::default()
        };
        assert_eq!(Design::from_config(&ui).background_speed, MAX_SPEED);
    }

    #[test]
    fn dim_and_opacity_levels_map_to_byte_endpoints() {
        let mut d = Design::from_config(&DesignConfig::default());
        d.dim = 0;
        assert_eq!(d.dim_alpha(), 255, "dim 0 fully blacks the wallpaper");
        d.dim = MAX_DIM;
        assert_eq!(d.dim_alpha(), 0, "dim MAX leaves the wallpaper untouched");
        d.background_opacity = 0;
        assert_eq!(
            d.background_opacity_byte(),
            0,
            "opacity 0 is wallpaper only"
        );
        d.background_opacity = MAX_OPACITY;
        assert_eq!(
            d.background_opacity_byte(),
            255,
            "opacity MAX is fully opaque"
        );
    }

    #[test]
    fn rosters_cover_every_variant() {
        assert_eq!(THEMES.len(), 14);
        assert_eq!(BORDERS.len(), 9);
        assert_eq!(BACKGROUNDS.len(), 6);
        assert!(title_fonts().len() > 100);
    }

    #[test]
    fn title_font_names_are_resolved_case_insensitively() {
        assert_eq!(normalize_title_font("ansi shadow"), DEFAULT_TITLE_FONT);
        assert_eq!(normalize_title_font("missing font"), DEFAULT_TITLE_FONT);
    }
}

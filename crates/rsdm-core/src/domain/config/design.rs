use serde::{Deserialize, Serialize};

pub const DEFAULT_TITLE_FONT: &str = "ANSI Shadow";

/// The look of one front (the greeter or the locker). It is the SAME type for
/// both, so `dm.design` and `lock.design` carry identical fields and can be
/// configured independently. The runtime switcher (the F1 menu) edits a live
/// copy of this; nothing is persisted.
///
/// The wallpaper fields (`wallpaper`, `wallpaper_dim`, `background_opacity`) are
/// honored only by the framebuffer locker; the TTY greeter has no wallpaper and
/// ignores them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DesignConfig {
    pub theme: ThemePreset,
    /// Frame style for the login box and the lock card.
    pub border_style: BorderStyle,
    /// Animated background painted behind the box.
    pub background: Background,
    /// Animation speed for the background, `0` (frozen) to `10` (fast). `5` is
    /// the natural rate; the value scales how fast the animation advances.
    pub background_speed: u8,
    /// What the centered banner shows.
    pub title_mode: TitleSource,
    /// Text for `title_mode = custom`. `None` falls back to `"RSDM"`.
    pub title_text: Option<String>,
    /// Logo for `title_mode = preset-logo`.
    pub title_preset: LogoPreset,
    /// The FIGlet typeface name used by figrs for the generated title banner.
    pub title_font: String,
    pub show_clock: bool,
    pub show_hostname: bool,
    pub password_mode: PasswordRendering,
    /// Whether the runtime design switcher (F1) is available on this front.
    /// Changes made through it apply live but are never persisted.
    pub menu: bool,
    /// Locker only: optional background image (PNG/JPEG, absolute path). The base
    /// layer of the lock screen; an animated background draws over it. The
    /// greeter ignores it.
    pub wallpaper: Option<String>,
    /// Locker only: how dark the wallpaper is drawn, `0` (fully black) to `10`
    /// (untouched). The greeter ignores it.
    pub wallpaper_dim: u8,
    /// Locker only: opacity of the animated `background` where it is drawn over a
    /// wallpaper, `0` (wallpaper only) to `10` (animation fully opaque). Ignored
    /// without a wallpaper, and by the greeter.
    pub background_opacity: u8,
}

impl Default for DesignConfig {
    fn default() -> Self {
        Self {
            theme: ThemePreset::Monochrome,
            border_style: BorderStyle::default(),
            background: Background::default(),
            background_speed: 5,
            title_mode: TitleSource::Custom,
            title_text: None,
            title_preset: LogoPreset::Rsdm,
            title_font: DEFAULT_TITLE_FONT.to_string(),
            show_clock: true,
            show_hostname: true,
            password_mode: PasswordRendering::Hidden,
            menu: true,
            wallpaper: None,
            wallpaper_dim: 6,
            background_opacity: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TitleSource {
    Custom,
    Hostname,
    OsRelease,
    SessionName,
    PresetLogo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LogoPreset {
    Minimal,
    Block,
    Rsdm,
    Rustty,
    Waytty,
    Niri,
    Hyprland,
    Kde,
    Gnome,
    Nixos,
    Arch,
    Tux,
}

/// A named color scheme. The same preset means the same colors everywhere. The
/// roster mirrors the looks shipped by sysc-greet; the colors live in
/// [`crate::domain::theme::palette`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemePreset {
    Minimal,
    Monochrome,
    Cyberpunk,
    Catppuccin,
    Gruvbox,
    Nord,
    Dracula,
    TokyoNight,
    Material,
    Solarized,
    Eldritch,
    Rama,
    Dark,
    TransIsHardJob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PasswordRendering {
    Hidden,
    Asterisks,
}

/// The frame drawn around the login box (and mirrored on the lock card). The
/// roster and names track sysc-greet's border styles. Every glyph stays inside
/// the 256-glyph console repertoire (box-drawing and the CP437 block run), so
/// the styles survive on a raw VT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BorderStyle {
    /// No frame or background panel around the content.
    #[serde(rename = "none")]
    None,
    /// Double outer frame around a single inner panel (sysc-greet default).
    #[default]
    #[serde(rename = "classic")]
    Classic,
    /// A single clean frame, nothing nested.
    #[serde(rename = "modern")]
    Modern,
    /// No frame glyphs, but keep the background panel.
    #[serde(rename = "minimal")]
    Minimal,
    /// Solid block border drawn from the half-block run.
    #[serde(rename = "ascii1")]
    Ascii1,
    /// Stepped block corners with a shaded gradient fade, no side rails.
    #[serde(rename = "ascii2")]
    Ascii2,
    /// Layered double frame with a shaded title bar inside.
    #[serde(rename = "ascii3")]
    Ascii3,
    /// Decorative block headers top and bottom, no side rails.
    #[serde(rename = "ascii4")]
    Ascii4,
    /// A wavy top and bottom edge with plain side rails.
    #[serde(rename = "wave")]
    Wave,
    /// Double inner frame inside a heavier outer frame.
    #[serde(rename = "pulse")]
    Pulse,
}

/// An animated backdrop, ported from sysc-greet's backgrounds. Each effect is a
/// pure function of `(x, y, frame)` so it animates without per-frame state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Background {
    /// A still, solid theme background.
    #[default]
    None,
    /// Falling digital-rain columns.
    Matrix,
    /// A flickering fire rising from the bottom edge.
    Fire,
    /// Sparse vertical rain streaks.
    Rain,
    /// A flowing truecolor plasma field.
    Plasma,
    /// A slow drifting starfield.
    Starfield,
}

impl Background {
    /// Whether this background needs a continuous redraw loop.
    pub fn is_animated(self) -> bool {
        !matches!(self, Background::None)
    }

    /// Whether the effect paints every cell (a full field) rather than sparse
    /// glyphs over a dark base. Full-field effects (plasma, fire) own the whole
    /// background color, so the locker lets them show behind text and frames
    /// instead of punching an opaque box. Sparse effects (matrix, rain,
    /// starfield) are dots over the theme background, which the box matches.
    pub fn is_full_field(self) -> bool {
        matches!(self, Background::Plasma | Background::Fire)
    }
}

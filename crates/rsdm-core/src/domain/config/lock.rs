use serde::{Deserialize, Serialize};

use super::DesignConfig;

/// Largest supported integer bitmap-glyph zoom for the lock screen.
pub const MAX_LOCK_SIZE: u8 = 12;

/// Locker settings. The look (including the wallpaper) lives in [`DesignConfig`]
/// (`lock.design`), the same type the greeter uses under `dm.design`. The locker
/// adds enable/authentication settings and the multi-output presentation policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LockConfig {
    /// Whether locking is possible at all. When `false`, `rsdm lock` refuses to
    /// lock the screen (so an idle daemon can be wired up without ever blanking).
    pub enable: bool,
    /// PAM service used to verify the current user's password. It must only run
    /// the `auth`/`account` stacks - a locker never opens a new session.
    pub pam_service: String,
    /// Output that carries the interactive unlock UI. Matches the Wayland
    /// output name (for example `DP-1`). When omitted, the output with the
    /// largest current pixel area is selected deterministically.
    pub primary_output: Option<String>,
    /// What secondary outputs show while the session is locked.
    pub secondary_output: SecondaryOutput,
    /// Integer zoom of the lock screen's 8x8 bitmap glyphs. `None` selects a
    /// TTY-like size automatically from the output's physical pixel height.
    /// Explicit values are useful when the console font or viewing distance is
    /// unusual.
    pub size: Option<u8>,
    /// The lock screen's look, configured separately from `dm.design`.
    pub design: DesignConfig,
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            enable: false,
            pam_service: "rsdm-lock".to_string(),
            primary_output: None,
            secondary_output: SecondaryOutput::Background,
            size: None,
            design: DesignConfig::default(),
        }
    }
}

/// Content shown on outputs other than [`LockConfig::primary_output`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecondaryOutput {
    /// Keep the configured wallpaper/background, without the clock, login card,
    /// hints, menu, or authentication status.
    #[default]
    Background,
    /// Paint an opaque black surface.
    Black,
    /// Ask a supported compositor to disable the output for the duration of the
    /// lock. Currently niri is supported; elsewhere this safely falls back to
    /// black while still covering the output.
    Off,
}

impl SecondaryOutput {
    pub const ALL: [Self; 3] = [Self::Background, Self::Black, Self::Off];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Black => "black",
            Self::Off => "off",
        }
    }
}

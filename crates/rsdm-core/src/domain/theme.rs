//! The one source of color truth, shared by every front end.
//!
//! A [`ThemePreset`] names a look; [`palette`] turns that name into a concrete
//! [`Palette`] of truecolor roles. The locker paints those `Rgb` values straight
//! into its framebuffer, and the greeter (ratatui) converts the same palette to
//! terminal colors - so a theme means the same thing on the VT and on the pixel
//! lock screen. The roles mirror sysc-greet's semantic palette so its themes
//! port over one-to-one.

use super::ThemePreset;

/// An 8-bit-per-channel opaque color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// Pure black, the target the locker dims a wallpaper toward.
    pub const BLACK: Rgb = Rgb::new(0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Build from a packed `0xRRGGBB` literal, so the theme table reads like the
    /// hex colors it was copied from.
    pub const fn hex(rgb: u32) -> Self {
        Self {
            r: (rgb >> 16) as u8,
            g: (rgb >> 8) as u8,
            b: rgb as u8,
        }
    }

    /// Pack into `0xAARRGGBB` with the given alpha, the layout Wayland's
    /// `Argb8888`/`Xrgb8888` shm formats expect on little-endian hosts.
    pub const fn argb(self, alpha: u8) -> u32 {
        (alpha as u32) << 24 | (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }

    /// The `(r, g, b)` channels, handy for front ends that take a color triple.
    pub const fn channels(self) -> (u8, u8, u8) {
        (self.r, self.g, self.b)
    }

    /// Linearly blend `self` over `other` by `t` in `0..=255` (0 = other).
    pub fn blend(self, other: Rgb, t: u8) -> Rgb {
        let mix = |a: u8, b: u8| {
            let a = a as u16;
            let b = b as u16;
            let t = t as u16;
            ((a * t + b * (255 - t)) / 255) as u8
        };
        Rgb::new(
            mix(self.r, other.r),
            mix(self.g, other.g),
            mix(self.b, other.b),
        )
    }
}

/// Semantic color roles for a theme, mirroring sysc-greet so its looks carry
/// over verbatim. Front ends pick roles by meaning ("the accent", "a danger")
/// rather than by raw color, which keeps every theme coherent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Screen background.
    pub bg_base: Rgb,
    /// Background of a selected/active row or an elevated surface.
    pub bg_active: Rgb,

    /// Primary brand color: focused fields, the prompt, key hints.
    pub primary: Rgb,
    /// Secondary brand color: the clock, secondary emphasis.
    pub secondary: Rgb,
    /// Accent: the inner frame, titles, the current selection.
    pub accent: Rgb,
    /// Caution (caps lock, repeated failures).
    pub warning: Rgb,
    /// Errors and the power menu.
    pub danger: Rgb,
    /// In-progress status ("Authenticating..."): the theme's cool blue.
    pub info: Rgb,

    /// Primary text.
    pub fg_primary: Rgb,
    /// Secondary text and field labels.
    pub fg_secondary: Rgb,
    /// Muted text: help footers and hints.
    pub fg_muted: Rgb,
    /// The faintest text, for de-emphasized detail.
    pub fg_subtle: Rgb,

    /// Default (unfocused) border.
    pub border_default: Rgb,
    /// Focused border.
    pub border_focus: Rgb,
}

/// Truecolor palette for `theme`. This is the single place colors are defined;
/// every front end reads from here.
pub fn palette(theme: ThemePreset) -> Palette {
    match theme {
        // rsdm originals -------------------------------------------------------
        ThemePreset::Minimal => Palette {
            bg_base: Rgb::hex(0x000000),
            bg_active: Rgb::hex(0x1a1a1a),
            primary: Rgb::hex(0xffffff),
            secondary: Rgb::hex(0xe5e5e5),
            accent: Rgb::hex(0xffffff),
            warning: Rgb::hex(0xd7af5f),
            danger: Rgb::hex(0xdc322f),
            info: Rgb::hex(0x87afd7),
            fg_primary: Rgb::hex(0xe5e5e5),
            fg_secondary: Rgb::hex(0xc8c8c8),
            fg_muted: Rgb::hex(0xb8b8b8),
            fg_subtle: Rgb::hex(0x808080),
            border_default: Rgb::hex(0x707070),
            border_focus: Rgb::hex(0xffffff),
        },
        ThemePreset::Cyberpunk => Palette {
            bg_base: Rgb::hex(0x070812),
            bg_active: Rgb::hex(0x16182a),
            primary: Rgb::hex(0x00eeff),
            secondary: Rgb::hex(0xff4081),
            accent: Rgb::hex(0xb8ff3d),
            warning: Rgb::hex(0xffd400),
            danger: Rgb::hex(0xff4081),
            info: Rgb::hex(0x4da6ff),
            fg_primary: Rgb::hex(0xeef5ff),
            fg_secondary: Rgb::hex(0xc8d2e6),
            fg_muted: Rgb::hex(0x707d99),
            fg_subtle: Rgb::hex(0x4a536b),
            border_default: Rgb::hex(0x585e80),
            border_focus: Rgb::hex(0x00eeff),
        },

        // sysc-greet roster ----------------------------------------------------
        ThemePreset::Monochrome => Palette {
            bg_base: Rgb::hex(0x1a1a1a),
            bg_active: Rgb::hex(0x2a2a2a),
            primary: Rgb::hex(0xffffff),
            secondary: Rgb::hex(0xcccccc),
            accent: Rgb::hex(0x888888),
            warning: Rgb::hex(0xaaaaaa),
            danger: Rgb::hex(0x999999),
            info: Rgb::hex(0x8fb8de),
            fg_primary: Rgb::hex(0xffffff),
            fg_secondary: Rgb::hex(0xcccccc),
            fg_muted: Rgb::hex(0xb0b0b0),
            fg_subtle: Rgb::hex(0x858585),
            border_default: Rgb::hex(0x555555),
            border_focus: Rgb::hex(0xffffff),
        },
        ThemePreset::Catppuccin => Palette {
            bg_base: Rgb::hex(0x1e1e2e),
            bg_active: Rgb::hex(0x313244),
            primary: Rgb::hex(0xcba6f7),
            secondary: Rgb::hex(0x89b4fa),
            accent: Rgb::hex(0xa6e3a1),
            warning: Rgb::hex(0xf9e2af),
            danger: Rgb::hex(0xf38ba8),
            info: Rgb::hex(0x89b4fa),
            fg_primary: Rgb::hex(0xcdd6f4),
            fg_secondary: Rgb::hex(0xbac2de),
            fg_muted: Rgb::hex(0xa6adc8),
            fg_subtle: Rgb::hex(0x585b70),
            border_default: Rgb::hex(0x313244),
            border_focus: Rgb::hex(0xcba6f7),
        },
        ThemePreset::Gruvbox => Palette {
            bg_base: Rgb::hex(0x282828),
            bg_active: Rgb::hex(0x3c3836),
            primary: Rgb::hex(0xfe8019),
            secondary: Rgb::hex(0x8ec07c),
            accent: Rgb::hex(0xfabd2f),
            warning: Rgb::hex(0xd79921),
            danger: Rgb::hex(0xcc241d),
            info: Rgb::hex(0x83a598),
            fg_primary: Rgb::hex(0xebdbb2),
            fg_secondary: Rgb::hex(0xd5c4a1),
            fg_muted: Rgb::hex(0xbdae93),
            fg_subtle: Rgb::hex(0xa89984),
            border_default: Rgb::hex(0x665c54),
            border_focus: Rgb::hex(0xfe8019),
        },
        ThemePreset::Nord => Palette {
            bg_base: Rgb::hex(0x2e3440),
            bg_active: Rgb::hex(0x3b4252),
            primary: Rgb::hex(0x81a1c1),
            secondary: Rgb::hex(0x88c0d0),
            accent: Rgb::hex(0x8fbcbb),
            warning: Rgb::hex(0xebcb8b),
            danger: Rgb::hex(0xbf616a),
            info: Rgb::hex(0x81a1c1),
            fg_primary: Rgb::hex(0xeceff4),
            fg_secondary: Rgb::hex(0xe5e9f0),
            fg_muted: Rgb::hex(0xd8dee9),
            fg_subtle: Rgb::hex(0x4c566a),
            border_default: Rgb::hex(0x3b4252),
            border_focus: Rgb::hex(0x81a1c1),
        },
        ThemePreset::Dracula => Palette {
            bg_base: Rgb::hex(0x282a36),
            bg_active: Rgb::hex(0x44475a),
            primary: Rgb::hex(0xbd93f9),
            secondary: Rgb::hex(0x8be9fd),
            accent: Rgb::hex(0x50fa7b),
            warning: Rgb::hex(0xf1fa8c),
            danger: Rgb::hex(0xff5555),
            info: Rgb::hex(0x8be9fd),
            fg_primary: Rgb::hex(0xf8f8f2),
            fg_secondary: Rgb::hex(0xf1f2f6),
            fg_muted: Rgb::hex(0x6272a4),
            fg_subtle: Rgb::hex(0x44475a),
            border_default: Rgb::hex(0x44475a),
            border_focus: Rgb::hex(0xbd93f9),
        },
        ThemePreset::TokyoNight => Palette {
            bg_base: Rgb::hex(0x1a1b26),
            bg_active: Rgb::hex(0x24283b),
            primary: Rgb::hex(0x7aa2f7),
            secondary: Rgb::hex(0xbb9af7),
            accent: Rgb::hex(0x9ece6a),
            warning: Rgb::hex(0xe0af68),
            danger: Rgb::hex(0xf7768e),
            info: Rgb::hex(0x7aa2f7),
            fg_primary: Rgb::hex(0xc0caf5),
            fg_secondary: Rgb::hex(0xa9b1d6),
            fg_muted: Rgb::hex(0x565f89),
            fg_subtle: Rgb::hex(0x414868),
            border_default: Rgb::hex(0x24283b),
            border_focus: Rgb::hex(0x7aa2f7),
        },
        ThemePreset::Material => Palette {
            bg_base: Rgb::hex(0x263238),
            bg_active: Rgb::hex(0x37474f),
            primary: Rgb::hex(0x80cbc4),
            secondary: Rgb::hex(0x64b5f6),
            accent: Rgb::hex(0xffab40),
            warning: Rgb::hex(0xffb300),
            danger: Rgb::hex(0xf44336),
            info: Rgb::hex(0x64b5f6),
            fg_primary: Rgb::hex(0xeceff1),
            fg_secondary: Rgb::hex(0xcfd8dc),
            fg_muted: Rgb::hex(0x90a4ae),
            fg_subtle: Rgb::hex(0x546e7a),
            border_default: Rgb::hex(0x37474f),
            border_focus: Rgb::hex(0x80cbc4),
        },
        ThemePreset::Solarized => Palette {
            bg_base: Rgb::hex(0x002b36),
            bg_active: Rgb::hex(0x073642),
            primary: Rgb::hex(0x268bd2),
            secondary: Rgb::hex(0x2aa198),
            accent: Rgb::hex(0x859900),
            warning: Rgb::hex(0xb58900),
            danger: Rgb::hex(0xdc322f),
            info: Rgb::hex(0x268bd2),
            fg_primary: Rgb::hex(0xfdf6e3),
            fg_secondary: Rgb::hex(0xeee8d5),
            fg_muted: Rgb::hex(0x93a1a1),
            fg_subtle: Rgb::hex(0x657b83),
            border_default: Rgb::hex(0x073642),
            border_focus: Rgb::hex(0x268bd2),
        },
        ThemePreset::Eldritch => Palette {
            bg_base: Rgb::hex(0x212337),
            bg_active: Rgb::hex(0x323449),
            primary: Rgb::hex(0x37f499),
            secondary: Rgb::hex(0x04d1f9),
            accent: Rgb::hex(0xa48cf2),
            warning: Rgb::hex(0xf1fc79),
            danger: Rgb::hex(0xf16c75),
            info: Rgb::hex(0x04d1f9),
            fg_primary: Rgb::hex(0xebfafa),
            fg_secondary: Rgb::hex(0xabb4da),
            fg_muted: Rgb::hex(0x7081d0),
            fg_subtle: Rgb::hex(0x3b4261),
            border_default: Rgb::hex(0x3b4261),
            border_focus: Rgb::hex(0x37f499),
        },
        ThemePreset::Rama => Palette {
            bg_base: Rgb::hex(0x2b2d42),
            bg_active: Rgb::hex(0x3b3d52),
            primary: Rgb::hex(0xef233c),
            secondary: Rgb::hex(0xd90429),
            accent: Rgb::hex(0xef233c),
            warning: Rgb::hex(0xf59e0b),
            danger: Rgb::hex(0xef233c),
            info: Rgb::hex(0x5c7cfa),
            fg_primary: Rgb::hex(0xedf2f4),
            fg_secondary: Rgb::hex(0x8d99ae),
            fg_muted: Rgb::hex(0x8d99ae),
            fg_subtle: Rgb::hex(0x6d7a8e),
            border_default: Rgb::hex(0x3b3d52),
            border_focus: Rgb::hex(0xef233c),
        },
        ThemePreset::Dark => Palette {
            bg_base: Rgb::hex(0x000000),
            bg_active: Rgb::hex(0x1a1a1a),
            primary: Rgb::hex(0xffffff),
            secondary: Rgb::hex(0xffffff),
            accent: Rgb::hex(0x808080),
            warning: Rgb::hex(0xaaaaaa),
            danger: Rgb::hex(0x999999),
            info: Rgb::hex(0x87afd7),
            fg_primary: Rgb::hex(0xffffff),
            fg_secondary: Rgb::hex(0xcccccc),
            fg_muted: Rgb::hex(0xb0b0b0),
            fg_subtle: Rgb::hex(0x858585),
            border_default: Rgb::hex(0x555555),
            border_focus: Rgb::hex(0xffffff),
        },
        ThemePreset::TransIsHardJob => Palette {
            bg_base: Rgb::hex(0x1a1a1a),
            bg_active: Rgb::hex(0x2a2a2a),
            primary: Rgb::hex(0x5bcefa),
            secondary: Rgb::hex(0xf5a9b8),
            accent: Rgb::hex(0xffffff),
            warning: Rgb::hex(0xf5a9b8),
            danger: Rgb::hex(0xff6b9d),
            info: Rgb::hex(0x5bcefa),
            fg_primary: Rgb::hex(0xffffff),
            fg_secondary: Rgb::hex(0xf5a9b8),
            fg_muted: Rgb::hex(0x5bcefa),
            fg_subtle: Rgb::hex(0x999999),
            border_default: Rgb::hex(0x444444),
            border_focus: Rgb::hex(0x5bcefa),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_unpacks_channels() {
        assert_eq!(Rgb::hex(0x123456), Rgb::new(0x12, 0x34, 0x56));
        assert_eq!(Rgb::hex(0xffffff), Rgb::new(255, 255, 255));
    }

    #[test]
    fn argb_packs_little_endian_layout() {
        assert_eq!(Rgb::new(0x12, 0x34, 0x56).argb(0xff), 0xff_12_34_56);
        assert_eq!(Rgb::new(0, 0, 0).argb(0), 0);
    }

    #[test]
    fn blend_endpoints_are_exact() {
        let a = Rgb::new(200, 100, 50);
        let b = Rgb::new(0, 0, 0);
        assert_eq!(a.blend(b, 255), a);
        assert_eq!(a.blend(b, 0), b);
    }

    #[test]
    fn every_preset_resolves() {
        // A smoke check that the match stays exhaustive as presets are added.
        for theme in [
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
        ] {
            let p = palette(theme);
            // Backgrounds must differ from primary text or the screen is unreadable.
            assert_ne!(p.bg_base, p.fg_primary);
        }
    }
}

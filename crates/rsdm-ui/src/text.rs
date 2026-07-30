//! Styled text segments keyed by semantic palette role, so the compositor names
//! colors by meaning ("a danger", "the accent") and never touches raw `Rgb`.

use rsdm_core::domain::{Palette, Rgb};

use crate::surface::Surface;

/// A semantic color slot, resolved against the active [`Palette`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Primary,
    Secondary,
    Accent,
    Warning,
    Danger,
    Info,
    FgPrimary,
    FgSecondary,
    FgMuted,
    FgSubtle,
    BorderFocus,
}

impl Role {
    pub fn color(self, p: Palette) -> Rgb {
        match self {
            Role::Primary => p.primary,
            Role::Secondary => p.secondary,
            Role::Accent => p.accent,
            Role::Warning => p.warning,
            Role::Danger => p.danger,
            Role::Info => p.info,
            Role::FgPrimary => p.fg_primary,
            Role::FgSecondary => p.fg_secondary,
            Role::FgMuted => p.fg_muted,
            Role::FgSubtle => p.fg_subtle,
            Role::BorderFocus => p.border_focus,
        }
    }
}

/// A run of text in one role.
#[derive(Debug, Clone)]
pub struct Segment {
    pub text: String,
    pub role: Role,
    pub bold: bool,
}

impl Segment {
    pub fn new(text: impl Into<String>, role: Role) -> Self {
        Self {
            text: text.into(),
            role,
            bold: false,
        }
    }

    pub fn bold(text: impl Into<String>, role: Role) -> Self {
        Self {
            text: text.into(),
            role,
            bold: true,
        }
    }
}

/// Total display width of a segment line, in cells.
pub fn segments_width(segments: &[Segment]) -> u16 {
    segments.iter().map(|s| s.text.chars().count() as u16).sum()
}

/// Draw `segments` left to right starting at `(x, y)`.
pub fn draw_segments(surface: &mut impl Surface, x: u16, y: u16, segments: &[Segment], p: Palette) {
    let mut cx = x;
    for seg in segments {
        surface.text(cx, y, &seg.text, seg.role.color(p), seg.bold);
        cx = cx.saturating_add(seg.text.chars().count() as u16);
    }
}

/// Draw `segments` centered horizontally around `center_x`.
pub fn draw_segments_centered(
    surface: &mut impl Surface,
    center_x: u16,
    y: u16,
    segments: &[Segment],
    p: Palette,
) {
    let width = segments_width(segments);
    let start = center_x.saturating_sub(width / 2);
    draw_segments(surface, start, y, segments, p);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::testing::VecSurface;
    use rsdm_core::domain::{ThemePreset, theme};

    #[test]
    fn segments_draw_in_sequence() {
        let mut s = VecSurface::new(20, 1);
        let p = theme::palette(ThemePreset::Nord);
        let segs = [
            Segment::new("ab", Role::Primary),
            Segment::new("cd", Role::Danger),
        ];
        draw_segments(&mut s, 0, 0, &segs, p);
        assert_eq!(s.glyph(0, 0), 'a');
        assert_eq!(s.glyph(2, 0), 'c');
        assert_eq!(segments_width(&segs), 4);
    }
}

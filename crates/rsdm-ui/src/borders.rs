//! The login/lock box frame, one implementation for both fronts.
//!
//! A frame is drawn into a cell rectangle and reports the interior rectangle the
//! body goes into. The nine styles track sysc-greet's border roster and are
//! drawn from box-drawing and the CP437 block run - glyphs both media can render
//! (the console font on the TTY; the `font8x8` BOX/BLOCK tables on the
//! framebuffer). This replaces the two old, divergent implementations
//! (`rsdm-tui/skins/borders.rs` drew a box with ratatui blocks; the locker
//! framed the whole screen). Now the locker shows the same centered box.

use rsdm_core::domain::{BorderStyle, Palette, Rgb};

use crate::surface::{Cell, Rect, Surface};

/// Box-drawing glyph set for a rectangular frame.
struct BoxSet {
    tl: char,
    tr: char,
    bl: char,
    br: char,
    h: char,
    v: char,
}

const LIGHT: BoxSet = BoxSet {
    tl: '\u{250c}',
    tr: '\u{2510}',
    bl: '\u{2514}',
    br: '\u{2518}',
    h: '\u{2500}',
    v: '\u{2502}',
};
const DOUBLE: BoxSet = BoxSet {
    tl: '\u{2554}',
    tr: '\u{2557}',
    bl: '\u{255a}',
    br: '\u{255d}',
    h: '\u{2550}',
    v: '\u{2551}',
};

const FULL: char = '\u{2588}';
const UPPER: char = '\u{2580}';
const LOWER: char = '\u{2584}';
const SHADE_DARK: char = '\u{2593}';
const SHADE_MED: char = '\u{2592}';

/// Padding from each frame edge to the interior content.
#[derive(Clone, Copy)]
struct Pad {
    x: u16,
    top: u16,
    bottom: u16,
}

fn pad(style: BorderStyle) -> Pad {
    match style {
        BorderStyle::Minimal => Pad {
            x: 0,
            top: 0,
            bottom: 0,
        },
        BorderStyle::Modern | BorderStyle::Wave => Pad {
            x: 2,
            top: 1,
            bottom: 1,
        },
        BorderStyle::Classic | BorderStyle::Pulse => Pad {
            x: 4,
            top: 2,
            bottom: 2,
        },
        BorderStyle::Ascii1 | BorderStyle::Ascii4 => Pad {
            x: 3,
            top: 2,
            bottom: 2,
        },
        BorderStyle::Ascii2 => Pad {
            x: 3,
            top: 3,
            bottom: 3,
        },
        BorderStyle::Ascii3 => Pad {
            x: 2,
            top: 3,
            bottom: 1,
        },
    }
}

/// The cell footprint a `content_w x content_h` body needs in `style`.
pub fn outer_size(style: BorderStyle, content_w: u16, content_h: u16) -> (u16, u16) {
    let p = pad(style);
    (content_w + 2 * p.x, content_h + p.top + p.bottom)
}

/// The interior content rectangle inside `frame` for `style`.
pub fn interior(style: BorderStyle, frame: Rect) -> Rect {
    let p = pad(style);
    Rect {
        x: frame.x + p.x,
        y: frame.y + p.top,
        w: frame.w.saturating_sub(2 * p.x),
        h: frame.h.saturating_sub(p.top + p.bottom),
    }
}

/// Fill `frame`'s background and draw the `style` decoration, returning the
/// interior content rectangle. The box is opaque so the body stays legible over
/// an animated background.
pub fn draw_frame(surface: &mut impl Surface, frame: Rect, style: BorderStyle, p: Palette) -> Rect {
    draw_frame_with_fill(surface, frame, style, p, true)
}

/// Draw a frame, optionally leaving the interior/background untouched. The
/// transparent mode is used by the framebuffer lock screen for plasma-like
/// effects where the animated pixels should remain visible behind glyphs.
pub fn draw_frame_with_fill(
    surface: &mut impl Surface,
    frame: Rect,
    style: BorderStyle,
    p: Palette,
    fill_background: bool,
) -> Rect {
    if fill_background {
        surface.fill(frame, p.bg_base);
    }
    match style {
        BorderStyle::Minimal => {}
        BorderStyle::Modern => draw_box(surface, frame, &LIGHT, p.primary),
        BorderStyle::Classic => {
            draw_box(surface, frame, &DOUBLE, p.border_default);
            draw_box(surface, frame.shrink(2, 1), &LIGHT, p.primary);
        }
        BorderStyle::Pulse => {
            draw_box(surface, frame, &DOUBLE, p.border_default);
            draw_box(surface, frame.shrink(2, 1), &DOUBLE, p.primary);
        }
        BorderStyle::Ascii1 => block_frame(surface, frame, p),
        BorderStyle::Ascii2 => gradient_frame(surface, frame, p),
        BorderStyle::Ascii3 => panel_frame(surface, frame, p),
        BorderStyle::Ascii4 => banner_frame(surface, frame, p),
        BorderStyle::Wave => wave_frame(surface, frame, p),
    }
    interior(style, frame)
}

// --- box styles -----------------------------------------------------------

fn draw_box(surface: &mut impl Surface, r: Rect, set: &BoxSet, color: Rgb) {
    if r.w < 2 || r.h < 2 {
        return;
    }
    let (x0, y0, x1, y1) = (r.x, r.y, r.right() - 1, r.bottom() - 1);
    surface.hrun(x0 + 1, y0, r.w - 2, set.h, color);
    surface.hrun(x0 + 1, y1, r.w - 2, set.h, color);
    surface.vrun(x0, y0 + 1, r.h - 2, set.v, color);
    surface.vrun(x1, y0 + 1, r.h - 2, set.v, color);
    surface.put(x0, y0, Cell::new(set.tl, color));
    surface.put(x1, y0, Cell::new(set.tr, color));
    surface.put(x0, y1, Cell::new(set.bl, color));
    surface.put(x1, y1, Cell::new(set.br, color));
}

// --- block-art styles -----------------------------------------------------

/// ascii1: a solid block border.
fn block_frame(surface: &mut impl Surface, r: Rect, p: Palette) {
    if r.w < 2 || r.h < 2 {
        return;
    }
    surface.hrun(r.x, r.y, r.w, UPPER, p.primary);
    surface.hrun(r.x, r.bottom() - 1, r.w, LOWER, p.primary);
    surface.vrun(r.x, r.y, r.h, FULL, p.primary);
    surface.vrun(r.right() - 1, r.y, r.h, FULL, p.primary);
}

/// ascii2: stepped gradient bars top and bottom, no side rails.
fn gradient_frame(surface: &mut impl Surface, r: Rect, p: Palette) {
    if r.h < 6 {
        return;
    }
    surface.hrun(r.x, r.y, r.w, LOWER, p.primary);
    surface.hrun(r.x, r.y + 1, r.w, SHADE_DARK, p.secondary);
    surface.hrun(r.x, r.y + 2, r.w, SHADE_MED, p.accent);
    surface.hrun(r.x, r.bottom() - 3, r.w, SHADE_MED, p.accent);
    surface.hrun(r.x, r.bottom() - 2, r.w, SHADE_DARK, p.secondary);
    surface.hrun(r.x, r.bottom() - 1, r.w, UPPER, p.primary);
}

/// ascii3: a layered double frame with a shaded title bar.
fn panel_frame(surface: &mut impl Surface, r: Rect, p: Palette) {
    if r.w < 4 || r.h < 4 {
        return;
    }
    let (x0, y0, x1, y1) = (r.x, r.y, r.right() - 1, r.bottom() - 1);
    // Outer double frame.
    surface.hrun(x0 + 1, y0, r.w - 2, DOUBLE.h, p.primary);
    surface.hrun(x0 + 1, y1, r.w - 2, DOUBLE.h, p.primary);
    surface.vrun(x0, y0 + 1, r.h - 2, DOUBLE.v, p.primary);
    surface.vrun(x1, y0 + 1, r.h - 2, DOUBLE.v, p.primary);
    surface.put(x0, y0, Cell::new(DOUBLE.tl, p.primary));
    surface.put(x1, y0, Cell::new(DOUBLE.tr, p.primary));
    surface.put(x0, y1, Cell::new(DOUBLE.bl, p.primary));
    surface.put(x1, y1, Cell::new(DOUBLE.br, p.primary));
    // Shaded title bar on row y0+1, separated below by a tee rule.
    let bar = "\u{2588}\u{2593}\u{2592}\u{2591} Greetings";
    surface.text(x0 + 2, y0 + 1, bar, p.secondary, true);
    surface.put(x0, y0 + 2, Cell::new('\u{2560}', p.accent));
    surface.hrun(x0 + 1, y0 + 2, r.w - 2, DOUBLE.h, p.accent);
    surface.put(x1, y0 + 2, Cell::new('\u{2563}', p.accent));
}

/// ascii4: solid block banners top and bottom, no side rails.
fn banner_frame(surface: &mut impl Surface, r: Rect, p: Palette) {
    if r.h < 4 {
        return;
    }
    surface.hrun(r.x, r.y, r.w, LOWER, p.secondary);
    surface.hrun(r.x, r.y + 1, r.w, FULL, p.primary);
    surface.hrun(r.x, r.bottom() - 2, r.w, FULL, p.primary);
    surface.hrun(r.x, r.bottom() - 1, r.w, UPPER, p.secondary);
}

/// wave: a wavy top and bottom edge with plain side rails.
fn wave_frame(surface: &mut impl Surface, r: Rect, p: Palette) {
    if r.w < 2 || r.h < 2 {
        return;
    }
    let (x0, y0, x1, y1) = (r.x, r.y, r.right() - 1, r.bottom() - 1);
    surface.hrun(x0 + 1, y0, r.w - 2, '~', p.secondary);
    surface.hrun(x0 + 1, y1, r.w - 2, '~', p.secondary);
    surface.vrun(x0, y0 + 1, r.h - 2, LIGHT.v, p.primary);
    surface.vrun(x1, y0 + 1, r.h - 2, LIGHT.v, p.primary);
    for &(x, y) in &[(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
        surface.put(x, y, Cell::new('+', p.secondary));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::testing::VecSurface;
    use rsdm_core::domain::{ThemePreset, theme};

    fn pal() -> Palette {
        theme::palette(ThemePreset::Nord)
    }

    #[test]
    fn interior_is_exactly_content_size() {
        for style in [
            BorderStyle::Classic,
            BorderStyle::Modern,
            BorderStyle::Minimal,
            BorderStyle::Ascii1,
            BorderStyle::Ascii2,
            BorderStyle::Ascii3,
            BorderStyle::Ascii4,
            BorderStyle::Wave,
            BorderStyle::Pulse,
        ] {
            let (w, h) = outer_size(style, 40, 12);
            let inner = interior(style, Rect::new(0, 0, w, h));
            assert_eq!((inner.w, inner.h), (40, 12), "{style:?}");
        }
    }

    #[test]
    fn classic_draws_a_double_corner() {
        let mut s = VecSurface::new(30, 8);
        let frame = Rect::new(0, 0, 30, 8);
        let inner = draw_frame(&mut s, frame, BorderStyle::Classic, pal());
        assert_eq!(s.glyph(0, 0), '\u{2554}'); // double top-left
        // Body area is inside the frame.
        assert!(inner.x >= 2 && inner.y >= 1);
    }

    #[test]
    fn every_style_draws_without_panic_small() {
        for style in [
            BorderStyle::Classic,
            BorderStyle::Modern,
            BorderStyle::Minimal,
            BorderStyle::Ascii1,
            BorderStyle::Ascii2,
            BorderStyle::Ascii3,
            BorderStyle::Ascii4,
            BorderStyle::Wave,
            BorderStyle::Pulse,
        ] {
            let mut s = VecSurface::new(12, 10);
            draw_frame(&mut s, Rect::new(0, 0, 12, 10), style, pal());
        }
    }
}

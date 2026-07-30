//! 8x8 bitmap glyphs, integer-scaled, from the pure-Rust `font8x8` crate.
//!
//! Lookups try ASCII first, then the block-element run (U+2580..U+259F) and the
//! box-drawing run (U+2500..U+257F), so the framebuffer can render the very same
//! frame and background glyphs the TTY uses. A glyph outside all three tables is
//! simply not drawn (the cell shows the background underneath).

use font8x8::{BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, UnicodeFonts};

use super::canvas::Canvas;

/// Native glyph size in pixels (before zooming).
pub const GLYPH: u32 = 8;

/// Look up an 8x8 glyph bitmap, trying ASCII, then block, then box drawing.
pub fn glyph(ch: char) -> Option<[u8; 8]> {
    BASIC_FONTS
        .get(ch)
        .or_else(|| BLOCK_FONTS.get(ch))
        .or_else(|| BOX_FONTS.get(ch))
}

/// Blit one 8x8 glyph at an integer `zoom`, its top-left corner at `(x, y)`.
///
/// Every source pixel becomes the exact same `zoom x zoom` block of destination
/// pixels, so the glyph fills precisely `GLYPH * zoom` pixels on each axis. That
/// is the whole trick, and the whole story: there is no fractional fitting, no
/// per-axis stretch, no "tile vs text" special case. Because every cell shares
/// the same `GLYPH * zoom` pitch, box-drawing and block glyphs line up edge to
/// edge and tile seamlessly, while plain text stays crisp at any zoom. Pixels
/// the glyph does not set are left untouched, so the background shows through
/// around and inside each letter.
pub fn draw_glyph(
    canvas: &mut Canvas,
    ch: char,
    x: i32,
    y: i32,
    zoom: u32,
    color: u32,
    opacity: u8,
) {
    let Some(bitmap) = glyph(ch) else {
        return;
    };
    let z = zoom.max(1);
    for (row, bits) in bitmap.iter().enumerate() {
        for col in 0..GLYPH {
            if bits & (1 << col) != 0 {
                canvas.fill_rect_alpha(
                    x + (col * z) as i32,
                    y + (row as u32 * z) as i32,
                    z,
                    z,
                    color,
                    opacity,
                );
            }
        }
    }
}

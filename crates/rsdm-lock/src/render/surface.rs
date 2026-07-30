//! [`rsdm_ui::Surface`] over the software framebuffer.
//!
//! One cell is a square `GLYPH * zoom` pixel block; the glyph inside is blitted
//! at the same integer `zoom` (see [`super::font`]), so text and box/block art
//! are always crisp and never stretched. The zoom is picked once from the output
//! height by [`resolve_zoom`] (or explicitly configured) and the grid is centered
//! in any leftover pixels. `fill`/`clear` paint pixel rectangles; `put` blits a
//! glyph over whatever is already there, leaving the background to show through
//! around and inside each letter. Truecolor is exact here - the framebuffer is
//! where the design has the most color fidelity.

use rsdm_core::domain::{MAX_LOCK_SIZE, Rgb};
use rsdm_ui::{Cell, Rect, Surface};

use super::canvas::Canvas;
use super::font::{GLYPH, draw_glyph};

/// Resolve the integer glyph zoom for an output. An explicit `size` maps one
/// source pixel to a `size x size` block. Automatic mode targets about 68 rows,
/// close to the density of a Linux console using its common 8x16 default font:
/// 1080p therefore uses a 16px cell (`zoom = 2`) rather than the former 24px
/// cell. Integer zoom keeps all bitmap glyphs and box drawing crisp.
pub fn resolve_zoom(height: u32, size: Option<u8>) -> u32 {
    size.map_or_else(
        || {
            const TARGET_ROWS: u32 = 68;
            let pixels_per_zoom = GLYPH * TARGET_ROWS;
            height
                .saturating_add(pixels_per_zoom / 2)
                .checked_div(pixels_per_zoom)
                .unwrap_or(1)
                .clamp(1, u32::from(MAX_LOCK_SIZE))
        },
        |size| u32::from(size.clamp(1, MAX_LOCK_SIZE)),
    )
}

pub struct FbSurface<'a> {
    canvas: &'a mut Canvas,
    zoom: u32,
    cell_px: u32,
    opacity: u8,
    cols: u16,
    rows: u16,
    off_x: i32,
    off_y: i32,
}

impl<'a> FbSurface<'a> {
    /// Wrap `canvas` as an opaque cell grid at integer glyph `zoom` (>= 1).
    pub fn new(canvas: &'a mut Canvas, zoom: u32) -> Self {
        Self::build(canvas, zoom, 255)
    }

    /// Wrap `canvas` as a cell grid whose writes are alpha-blended. Used for the
    /// lock background layer when it is blended over a wallpaper; the UI layer
    /// stays fully opaque.
    pub fn with_opacity(canvas: &'a mut Canvas, zoom: u32, opacity: u8) -> Self {
        Self::build(canvas, zoom, opacity)
    }

    fn build(canvas: &'a mut Canvas, zoom: u32, opacity: u8) -> Self {
        let zoom = zoom.max(1);
        let cell_px = GLYPH * zoom;
        let cols = (canvas.width() / cell_px) as u16;
        let rows = (canvas.height() / cell_px) as u16;
        let off_x = ((canvas.width() - cols as u32 * cell_px) / 2) as i32;
        let off_y = ((canvas.height() - rows as u32 * cell_px) / 2) as i32;
        Self {
            canvas,
            zoom,
            cell_px,
            opacity,
            cols,
            rows,
            off_x,
            off_y,
        }
    }

    fn px(&self, cx: u16) -> i32 {
        self.off_x + cx as i32 * self.cell_px as i32
    }

    fn py(&self, cy: u16) -> i32 {
        self.off_y + cy as i32 * self.cell_px as i32
    }
}

impl Surface for FbSurface<'_> {
    fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    fn clear(&mut self, bg: Rgb) {
        let (w, h) = (self.canvas.width(), self.canvas.height());
        self.canvas
            .fill_rect_alpha(0, 0, w, h, bg.argb(0xff), self.opacity);
    }

    fn fill(&mut self, rect: Rect, bg: Rgb) {
        let (x, y) = (self.px(rect.x), self.py(rect.y));
        let (w, h) = (rect.w as u32 * self.cell_px, rect.h as u32 * self.cell_px);
        self.canvas
            .fill_rect_alpha(x, y, w, h, bg.argb(0xff), self.opacity);
    }

    fn put(&mut self, x: u16, y: u16, cell: Cell) {
        if x >= self.cols || y >= self.rows {
            return;
        }
        draw_glyph(
            self.canvas,
            cell.ch,
            self.px(x),
            self.py(y),
            self.zoom,
            cell.fg.argb(0xff),
            self.opacity,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_dimensions_follow_zoom() {
        let mut canvas = Canvas::try_new(800, 600, 0).expect("canvas");
        let cell = GLYPH * 4;
        let surface = FbSurface::new(&mut canvas, 4);
        assert_eq!(surface.size(), ((800 / cell) as u16, (600 / cell) as u16));
    }

    #[test]
    fn zoom_scales_with_height_and_stays_sane() {
        assert_eq!(resolve_zoom(1080, None), 2);
        assert_eq!(resolve_zoom(1440, None), 3);
        assert_eq!(resolve_zoom(2160, None), 4);
        assert_eq!(resolve_zoom(240, None), 1);
        assert_eq!(resolve_zoom(99_999, None), 12);
        assert_eq!(resolve_zoom(2160, Some(2)), 2);
        assert_eq!(resolve_zoom(1080, Some(0)), 1);
        assert_eq!(resolve_zoom(1080, Some(99)), 12);
    }
}

//! [`rsdm_ui::Surface`] over the software framebuffer.
//!
//! Cells use the console font's actual width and height in physical pixels.
//! A single integer zoom applies equally to both axes.

use rsdm_core::domain::{MAX_LOCK_SIZE, Rgb};
use rsdm_ui::{Cell, Rect, Surface};

use super::canvas::Canvas;
use super::font::Font;

/// Auto uses the Greeter's native pixel size, independent of output resolution.
pub fn resolve_zoom(size: Option<u8>) -> u32 {
    u32::from(size.unwrap_or(1).clamp(1, MAX_LOCK_SIZE))
}

pub struct FbSurface<'a> {
    canvas: &'a mut Canvas,
    zoom: u32,
    font: &'a Font,
    cell_width: u32,
    cell_height: u32,
    opacity: u8,
    cols: u16,
    rows: u16,
}

impl<'a> FbSurface<'a> {
    /// Wrap `canvas` as an opaque cell grid at integer glyph `zoom` (>= 1).
    pub fn new(canvas: &'a mut Canvas, font: &'a Font, zoom: u32) -> Self {
        Self::build(canvas, font, zoom, 255)
    }

    /// Wrap `canvas` as a cell grid whose writes are alpha-blended. Used for the
    /// lock background layer when it is blended over a wallpaper; the UI layer
    /// stays fully opaque.
    pub fn with_opacity(canvas: &'a mut Canvas, font: &'a Font, zoom: u32, opacity: u8) -> Self {
        Self::build(canvas, font, zoom, opacity)
    }

    fn build(canvas: &'a mut Canvas, font: &'a Font, zoom: u32, opacity: u8) -> Self {
        let zoom = zoom.max(1);
        let cell_width = font.width() * zoom;
        let cell_height = font.height() * zoom;
        let cols = (canvas.width() / cell_width) as u16;
        let rows = (canvas.height() / cell_height) as u16;
        Self {
            canvas,
            zoom,
            font,
            cell_width,
            cell_height,
            opacity,
            cols,
            rows,
        }
    }

    fn px(&self, cx: u16) -> i32 {
        cx as i32 * self.cell_width as i32
    }

    fn py(&self, cy: u16) -> i32 {
        cy as i32 * self.cell_height as i32
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
        let (w, h) = (
            rect.w as u32 * self.cell_width,
            rect.h as u32 * self.cell_height,
        );
        self.canvas
            .fill_rect_alpha(x, y, w, h, bg.argb(0xff), self.opacity);
    }

    fn put(&mut self, x: u16, y: u16, cell: Cell) {
        if x >= self.cols || y >= self.rows {
            return;
        }
        self.font.draw(
            self.canvas,
            cell.ch,
            (self.px(x), self.py(y)),
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
    fn grid_dimensions_follow_console_cell_proportions() {
        let mut canvas = Canvas::try_new(800, 600, 0).expect("canvas");
        let font = Font::default();
        let surface = FbSurface::new(&mut canvas, &font, 1);
        assert_eq!(surface.size(), (100, 37));
    }

    #[test]
    fn auto_zoom_preserves_native_font_size() {
        assert_eq!(resolve_zoom(None), 1);
        assert_eq!(resolve_zoom(Some(2)), 2);
        assert_eq!(resolve_zoom(Some(0)), 1);
        assert_eq!(resolve_zoom(Some(99)), 12);
    }

    #[test]
    fn cells_start_at_console_origin_and_tile_without_gaps() {
        let mut canvas = Canvas::try_new(19, 35, 0).unwrap();
        let font = Font::default();
        let mut surface = FbSurface::new(&mut canvas, &font, 1);
        surface.put(0, 0, Cell::new('█', Rgb::hex(0xffffff)));
        surface.put(1, 0, Cell::new('█', Rgb::hex(0xffffff)));
        surface.fill(Rect::new(0, 1, 2, 1), Rgb::hex(0xffffff));
        for y in 0..35 {
            for x in 0..19 {
                assert_eq!(canvas.pixels()[y * 19 + x] != 0, x < 16 && y < 32);
            }
        }
    }
}

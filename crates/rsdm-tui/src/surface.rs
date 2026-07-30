//! [`rsdm_ui::Surface`] over ratatui's cell buffer.
//!
//! This is the greeter's whole backend: it turns the shared composition's cell
//! ops into ratatui cells. Truecolor is passed straight through as
//! `Color::Rgb`; the terminal/console degrades it as it sees fit.

use ratatui::{
    buffer::Buffer,
    layout::Position,
    style::{Color, Modifier},
};
use rsdm_core::domain::Rgb;
use rsdm_ui::{Cell, Rect, Surface};

pub struct TuiSurface<'a> {
    buf: &'a mut Buffer,
}

impl<'a> TuiSurface<'a> {
    pub fn new(buf: &'a mut Buffer) -> Self {
        Self { buf }
    }
}

fn color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

impl Surface for TuiSurface<'_> {
    fn size(&self) -> (u16, u16) {
        (self.buf.area.width, self.buf.area.height)
    }

    fn clear(&mut self, bg: Rgb) {
        let area = self.buf.area;
        self.fill(Rect::new(0, 0, area.width, area.height), bg);
    }

    fn fill(&mut self, rect: Rect, bg: Rgb) {
        let area = self.buf.area;
        let c = color(bg);
        let x0 = area.x.saturating_add(rect.x);
        let y0 = area.y.saturating_add(rect.y);
        let x1 = x0.saturating_add(rect.w).min(area.x + area.width);
        let y1 = y0.saturating_add(rect.h).min(area.y + area.height);
        for y in y0..y1 {
            for x in x0..x1 {
                if let Some(cell) = self.buf.cell_mut(Position::new(x, y)) {
                    cell.reset();
                    cell.set_symbol(" ");
                    cell.set_bg(c);
                }
            }
        }
    }

    fn put(&mut self, x: u16, y: u16, cell: Cell) {
        let area = self.buf.area;
        let ax = area.x.saturating_add(x);
        let ay = area.y.saturating_add(y);
        if ax >= area.x + area.width || ay >= area.y + area.height {
            return;
        }
        if let Some(target) = self.buf.cell_mut(Position::new(ax, ay)) {
            let mut utf8 = [0u8; 4];
            target.set_symbol(cell.ch.encode_utf8(&mut utf8));
            target.set_fg(color(cell.fg));
            if cell.bold {
                target.modifier.insert(Modifier::BOLD);
            }
        }
    }
}

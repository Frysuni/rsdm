//! Grid drawing primitives shared by terminal and framebuffer backends.

use rsdm_core::domain::Rgb;

/// One styled glyph cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Rgb,
    pub bold: bool,
}

impl Cell {
    /// A plain glyph (frames, backgrounds, banner art, runs).
    pub const fn new(ch: char, fg: Rgb) -> Self {
        Self {
            ch,
            fg,
            bold: false,
        }
    }

    /// A bold glyph.
    pub const fn bold(ch: char, fg: Rgb) -> Self {
        Self { ch, fg, bold: true }
    }

    /// A glyph with an explicit weight.
    pub const fn text(ch: char, fg: Rgb, bold: bool) -> Self {
        Self { ch, fg, bold }
    }
}

/// A rectangle in cell space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }

    pub const fn right(&self) -> u16 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> u16 {
        self.y + self.h
    }

    pub const fn center_x(&self) -> u16 {
        self.x + self.w / 2
    }

    pub const fn center_y(&self) -> u16 {
        self.y + self.h / 2
    }

    /// Shrink by `dx` columns on each side and `dy` rows on each side, never
    /// past zero size.
    pub fn shrink(&self, dx: u16, dy: u16) -> Rect {
        Rect {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            w: self.w.saturating_sub(dx.saturating_mul(2)),
            h: self.h.saturating_sub(dy.saturating_mul(2)),
        }
    }

    /// A `w x h` rect centered inside `self`, clamped to `self`'s size.
    pub fn centered(&self, w: u16, h: u16) -> Rect {
        let w = w.min(self.w);
        let h = h.min(self.h);
        Rect {
            x: self.x + (self.w - w) / 2,
            y: self.y + (self.h - h) / 2,
            w,
            h,
        }
    }
}

/// An abstract grid of styled cells. Implementors provide the four primitives;
/// the text helpers are shared.
pub trait Surface {
    /// `(cols, rows)`.
    fn size(&self) -> (u16, u16);

    /// Paint the whole surface with a solid background.
    fn clear(&mut self, bg: Rgb);

    /// Paint a rectangle's background, clipped to the surface.
    fn fill(&mut self, rect: Rect, bg: Rgb);

    /// Stamp one foreground glyph, clipped to the surface.
    fn put(&mut self, x: u16, y: u16, cell: Cell);

    fn cols(&self) -> u16 {
        self.size().0
    }

    fn rows(&self) -> u16 {
        self.size().1
    }

    /// The full surface as a rect.
    fn area(&self) -> Rect {
        let (w, h) = self.size();
        Rect::new(0, 0, w, h)
    }

    /// Draw a left-aligned string starting at `(x, y)`. Cells past the right
    /// edge are clipped by [`Surface::put`].
    fn text(&mut self, x: u16, y: u16, text: &str, fg: Rgb, bold: bool) {
        let cols = self.cols();
        for (offset, ch) in text.chars().enumerate() {
            let cx = x.saturating_add(offset as u16);
            if cx >= cols {
                break;
            }
            self.put(cx, y, Cell::text(ch, fg, bold));
        }
    }

    /// Draw `text` centered horizontally around `center_x`.
    fn text_centered(&mut self, center_x: u16, y: u16, text: &str, fg: Rgb, bold: bool) {
        let width = text.chars().count() as u16;
        let start = center_x.saturating_sub(width / 2);
        self.text(start, y, text, fg, bold);
    }

    /// Repeat `ch` horizontally for `len` cells from `(x, y)`.
    fn hrun(&mut self, x: u16, y: u16, len: u16, ch: char, fg: Rgb) {
        for i in 0..len {
            self.put(x.saturating_add(i), y, Cell::new(ch, fg));
        }
    }

    /// Repeat `ch` vertically for `len` cells from `(x, y)`.
    fn vrun(&mut self, x: u16, y: u16, len: u16, ch: char, fg: Rgb) {
        for i in 0..len {
            self.put(x, y.saturating_add(i), Cell::new(ch, fg));
        }
    }
}

/// The display width of `text`, in cells.
pub fn text_width(text: &str) -> u16 {
    text.chars().count() as u16
}

#[cfg(test)]
pub(crate) mod testing {
    //! A trivial in-memory [`Surface`] so the shared modules can be unit-tested
    //! without any front end.

    use super::*;

    pub struct VecSurface {
        w: u16,
        h: u16,
        pub cells: Vec<Option<Cell>>,
        pub bg: Vec<Rgb>,
    }

    impl VecSurface {
        pub fn new(w: u16, h: u16) -> Self {
            Self {
                w,
                h,
                cells: vec![None; w as usize * h as usize],
                bg: vec![Rgb::new(0, 0, 0); w as usize * h as usize],
            }
        }

        fn idx(&self, x: u16, y: u16) -> usize {
            y as usize * self.w as usize + x as usize
        }

        /// The glyph at a cell, or a space if none was stamped.
        pub fn glyph(&self, x: u16, y: u16) -> char {
            self.cells[self.idx(x, y)].map_or(' ', |c| c.ch)
        }

        pub fn dump(&self) -> String {
            let mut output = String::new();
            for y in 0..self.h {
                for x in 0..self.w {
                    output.push(self.glyph(x, y));
                }
                output.push('\n');
            }
            output
        }
    }

    impl Surface for VecSurface {
        fn size(&self) -> (u16, u16) {
            (self.w, self.h)
        }

        fn clear(&mut self, bg: Rgb) {
            for pixel in &mut self.bg {
                *pixel = bg;
            }
            for cell in &mut self.cells {
                *cell = None;
            }
        }

        fn fill(&mut self, rect: Rect, bg: Rgb) {
            for y in rect.y..rect.bottom().min(self.h) {
                for x in rect.x..rect.right().min(self.w) {
                    let i = self.idx(x, y);
                    self.bg[i] = bg;
                }
            }
        }

        fn put(&mut self, x: u16, y: u16, cell: Cell) {
            if x < self.w && y < self.h {
                let i = self.idx(x, y);
                self.cells[i] = Some(cell);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::VecSurface;
    use super::*;

    #[test]
    fn text_clips_at_the_right_edge() {
        let mut s = VecSurface::new(5, 1);
        s.text(3, 0, "hello", Rgb::new(255, 255, 255), false);
        // Only the first two glyphs fit (cols 3 and 4).
        assert_eq!(s.glyph(3, 0), 'h');
        assert_eq!(s.glyph(4, 0), 'e');
    }

    #[test]
    fn centered_text_is_centered() {
        let mut s = VecSurface::new(11, 1);
        s.text_centered(5, 0, "abc", Rgb::new(1, 2, 3), false);
        assert_eq!(s.glyph(4, 0), 'a');
        assert_eq!(s.glyph(5, 0), 'b');
        assert_eq!(s.glyph(6, 0), 'c');
    }

    #[test]
    fn rect_centered_clamps_to_outer() {
        let outer = Rect::new(0, 0, 10, 4);
        let inner = outer.centered(4, 2);
        assert_eq!(inner, Rect::new(3, 1, 4, 2));
        let clamped = outer.centered(20, 20);
        assert_eq!(clamped, Rect::new(0, 0, 10, 4));
    }
}

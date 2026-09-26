//! Rasterize the Greeter's console bitmaps without changing their proportions.

use font8x8::{BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, UnicodeFonts};
use rsdm_infra::console_font::{self, ConsoleFont};

use super::canvas::Canvas;

#[derive(Default)]
pub struct Font {
    console: Option<ConsoleFont>,
}

impl Font {
    pub fn load(tty: &str) -> Self {
        let console = match console_font::load(tty) {
            Ok(font) => {
                tracing::info!(
                    width = font.width(),
                    height = font.height(),
                    "using Greeter console font"
                );
                Some(font)
            }
            Err(error) => {
                tracing::warn!(%error, "Greeter font unavailable; using fallback 8x16 font");
                None
            }
        };
        Self { console }
    }

    pub fn width(&self) -> u32 {
        self.console.as_ref().map_or(8, ConsoleFont::width)
    }

    pub fn height(&self) -> u32 {
        self.console.as_ref().map_or(16, ConsoleFont::height)
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas,
        ch: char,
        position: (i32, i32),
        zoom: u32,
        color: u32,
        opacity: u8,
    ) {
        let fallback;
        let bitmap = if let Some(font) = &self.console {
            let Some(bitmap) = font.glyph(ch) else { return };
            bitmap
        } else {
            fallback = fallback_glyph(ch);
            &fallback
        };
        let stride = self.width().div_ceil(8) as usize;
        for row in 0..self.height() {
            for col in 0..self.width() {
                if bitmap[row as usize * stride + col as usize / 8] & (0x80 >> (col % 8)) != 0 {
                    canvas.fill_rect_alpha(
                        position.0 + (col * zoom) as i32,
                        position.1 + (row * zoom) as i32,
                        zoom,
                        zoom,
                        color,
                        opacity,
                    );
                }
            }
        }
    }
}

fn fallback_glyph(ch: char) -> [u8; 16] {
    let bitmap = BASIC_FONTS
        .get(ch)
        .or_else(|| BLOCK_FONTS.get(ch))
        .or_else(|| BOX_FONTS.get(ch))
        .or_else(|| BASIC_FONTS.get('?'))
        .unwrap_or([0; 8]);
    std::array::from_fn(|row| bitmap[row / 2].reverse_bits())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_bitmap_is_pixel_exact_at_native_size_and_integer_zoom() {
        // A 12x3 asymmetric glyph crosses a byte boundary. Padding bits must
        // not leak into the next cell, and bit order must match the console.
        let bitmap = [0x80, 0x1f, 0x40, 0x2f, 0x20, 0x4f];
        let mut snapshot = b"RSDMFNT1".to_vec();
        for value in [12_u32, 3, 6, 1] {
            snapshot.extend_from_slice(&value.to_le_bytes());
        }
        snapshot.extend_from_slice(&bitmap);
        snapshot.extend_from_slice(&('A' as u32).to_le_bytes());
        snapshot.extend_from_slice(&0_u32.to_le_bytes());
        let font = Font {
            console: Some(ConsoleFont::from_snapshot(&snapshot).unwrap()),
        };

        for zoom in [1, 2] {
            let mut canvas = Canvas::try_new(30, 10, 0).unwrap();
            font.draw(&mut canvas, 'A', (1, 1), zoom, 0xffff_ffff, 255);
            for y in 0..10 {
                for x in 0..30 {
                    let expected = [(0, 0), (11, 0), (1, 1), (10, 1), (2, 2), (9, 2)]
                        .iter()
                        .any(|&(gx, gy)| {
                            (1 + gx * zoom..1 + (gx + 1) * zoom).contains(&x)
                                && (1 + gy * zoom..1 + (gy + 1) * zoom).contains(&y)
                        });
                    assert_eq!(canvas.pixels()[(y * 30 + x) as usize] != 0, expected);
                }
            }
        }
    }

    #[test]
    fn fallback_preserves_rectangular_console_cells() {
        let font = Font { console: None };
        assert_eq!((font.width(), font.height()), (8, 16));
        assert_eq!(fallback_glyph('█'), [255; 16]);
    }
}

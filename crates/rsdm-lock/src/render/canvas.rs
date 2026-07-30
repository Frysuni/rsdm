//! A tiny software framebuffer in `0xAARRGGBB` (matches Wayland `Argb8888` on
//! little-endian hosts).

use anyhow::{Context, Result};

/// A single output should never need more than this (an 8K ARGB buffer is
/// roughly 127 MiB). Bounding it avoids an OOM abort if a compositor sends
/// corrupt or hostile surface dimensions.
pub const MAX_CANVAS_BYTES: usize = 256 * 1024 * 1024;

/// An ARGB pixel buffer.
pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
}

impl Canvas {
    pub fn try_new(width: u32, height: u32, fill: u32) -> Result<Self> {
        let len = (width as usize)
            .checked_mul(height as usize)
            .context("canvas pixel count overflow")?;
        let bytes = len
            .checked_mul(std::mem::size_of::<u32>())
            .context("canvas byte count overflow")?;
        anyhow::ensure!(
            bytes <= MAX_CANVAS_BYTES,
            "canvas requires {bytes} bytes (limit is {MAX_CANVAS_BYTES})"
        );
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(len)
            .context("allocating canvas pixels")?;
        pixels.resize(len, fill);
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    #[cfg(test)]
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// Unpack to interleaved RGBA8, for PNG previews and screenshots.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for pixel in &self.pixels {
            out.push(((pixel >> 16) & 0xff) as u8);
            out.push(((pixel >> 8) & 0xff) as u8);
            out.push((pixel & 0xff) as u8);
            out.push(((pixel >> 24) & 0xff) as u8);
        }
        out
    }

    pub fn pixels_mut(&mut self) -> &mut [u32] {
        &mut self.pixels
    }

    /// Reinterpret the buffer as bytes for an shm copy. The `u32` layout is
    /// native-endian, which is what the Wayland shm buffer also expects.
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: u32 has no padding and any bit pattern is a valid u32; the
        // slice covers exactly the same allocation for its full lifetime.
        unsafe {
            std::slice::from_raw_parts(self.pixels.as_ptr().cast::<u8>(), self.pixels.len() * 4)
        }
    }

    #[inline]
    pub fn put(&mut self, x: u32, y: u32, color: u32) {
        if x < self.width && y < self.height {
            let index = (y as usize) * (self.width as usize) + (x as usize);
            self.pixels[index] = color;
        }
    }

    /// Fill an axis-aligned rectangle, clipped to the canvas.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: u32) {
        let (x0, y0, x1, y1) = self.clip(x, y, w, h);
        for py in y0..y1 {
            let row = (py as usize) * (self.width as usize);
            for px in x0..x1 {
                self.pixels[row + px as usize] = color;
            }
        }
    }

    /// Fill an axis-aligned rectangle, alpha-blending `color` over existing
    /// pixels. `opacity = 255` is the same as [`Canvas::fill_rect`].
    pub fn fill_rect_alpha(&mut self, x: i32, y: i32, w: u32, h: u32, color: u32, opacity: u8) {
        if opacity == 255 {
            self.fill_rect(x, y, w, h, color);
            return;
        }
        if opacity == 0 {
            return;
        }
        let (x0, y0, x1, y1) = self.clip(x, y, w, h);
        for py in y0..y1 {
            let row = (py as usize) * (self.width as usize);
            for px in x0..x1 {
                let dst = &mut self.pixels[row + px as usize];
                *dst = blend_argb(color, *dst, opacity);
            }
        }
    }

    fn clip(&self, x: i32, y: i32, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let x0 = x.max(0).min(self.width as i32) as u32;
        let y0 = y.max(0).min(self.height as i32) as u32;
        let x1 = (x + w as i32).max(0).min(self.width as i32) as u32;
        let y1 = (y + h as i32).max(0).min(self.height as i32) as u32;
        (x0, y0, x1.max(x0), y1.max(y0))
    }
}

fn blend_argb(src: u32, dst: u32, opacity: u8) -> u32 {
    let blend = |shift: u32| {
        let s = ((src >> shift) & 0xff) as u16;
        let d = ((dst >> shift) & 0xff) as u16;
        let a = opacity as u16;
        ((s * a + d * (255 - a)) / 255) as u32
    };
    0xff00_0000 | (blend(16) << 16) | (blend(8) << 8) | blend(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_rect_is_clipped() {
        let mut canvas = Canvas::try_new(4, 4, 0).expect("canvas");
        canvas.fill_rect(-2, -2, 3, 3, 0xffffffff);
        assert_eq!(canvas.pixels()[0], 0xffffffff);
        // The bottom-right corner stays untouched.
        assert_eq!(canvas.pixels()[15], 0);
    }

    #[test]
    fn as_bytes_has_four_per_pixel() {
        let canvas = Canvas::try_new(2, 3, 0).expect("canvas");
        assert_eq!(canvas.as_bytes().len(), 2 * 3 * 4);
    }

    #[test]
    fn fill_rect_alpha_blends_over_existing_pixels() {
        let mut canvas = Canvas::try_new(1, 1, 0xff00_0000).expect("canvas");
        canvas.fill_rect_alpha(0, 0, 1, 1, 0xffff_ffff, 128);
        let pixel = canvas.pixels()[0];
        assert_eq!((pixel >> 24) & 0xff, 0xff);
        assert!(((pixel >> 16) & 0xff) >= 127);
        assert!(((pixel >> 8) & 0xff) >= 127);
        assert!((pixel & 0xff) >= 127);
    }

    #[test]
    fn unreasonable_canvas_is_rejected_before_allocation() {
        assert!(Canvas::try_new(u32::MAX, u32::MAX, 0).is_err());
        assert!(Canvas::try_new(16_384, 16_384, 0).is_err());
    }
}

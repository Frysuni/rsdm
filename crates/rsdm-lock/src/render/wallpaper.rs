//! Background image loading and cover-fit blitting.

use std::path::Path;

use rsdm_core::domain::Rgb;

use super::canvas::Canvas;

/// Blend every pixel toward `target` by `alpha` (0 keeps the wallpaper, 255
/// fully replaces it with `target`). The locker's wallpaper-dim knob; with a
/// black target, `alpha` is how dark the wallpaper is drawn.
pub fn tint(canvas: &mut Canvas, target: Rgb, alpha: u8) {
    if alpha == 0 {
        return;
    }
    for pixel in canvas.pixels_mut() {
        let r = ((*pixel >> 16) & 0xff) as u8;
        let g = ((*pixel >> 8) & 0xff) as u8;
        let b = (*pixel & 0xff) as u8;
        // `blend(self, other, t)` returns `self` at t=255: weight the target by
        // alpha so alpha=255 is fully target and small alpha barely tints.
        let blended = target.blend(Rgb::new(r, g, b), alpha);
        *pixel = blended.argb(0xff);
    }
}

/// A decoded RGBA8 wallpaper.
pub struct Wallpaper {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Wallpaper {
    pub fn load(path: &Path) -> Result<Self, image::ImageError> {
        let image = image::open(path)?.to_rgba8();
        Ok(Self {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        })
    }

    /// The caller owns one cache per output for this immutable wallpaper.
    /// Dim and animated effects are applied after copying the prepared image.
    pub fn cover_cached(&self, canvas: &mut Canvas, cache: &mut Option<Canvas>) {
        if self.width == 0 || self.height == 0 { return; }
        let size = (canvas.width(), canvas.height());
        if !cache.as_ref().is_some_and(|image| (image.width(), image.height()) == size) {
            // A resized image has no further use; free it before allocating.
            *cache = None;
            let mut image = match Canvas::try_new(size.0, size.1, 0) {
                Ok(image) => image,
                Err(error) => {
                    tracing::debug!(%error, "wallpaper cache unavailable; resampling this frame");
                    self.cover_into(canvas);
                    return;
                }
            };
            self.cover_into(&mut image);
            *cache = Some(image);
        }
        if let Some(image) = cache {
            canvas.pixels_mut().copy_from_slice(image.pixels());
        }
    }

    /// Scale to fully cover `canvas` (center-cropping the overflow) and blit as
    /// opaque pixels.
    pub fn cover_into(&self, canvas: &mut Canvas) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let dst_w = canvas.width() as f64;
        let dst_h = canvas.height() as f64;
        let scale = (dst_w / self.width as f64).max(dst_h / self.height as f64);
        let scaled_w = self.width as f64 * scale;
        let scaled_h = self.height as f64 * scale;
        let off_x = (dst_w - scaled_w) / 2.0;
        let off_y = (dst_h - scaled_h) / 2.0;

        for dy in 0..canvas.height() {
            let sy = (((dy as f64) - off_y) / scale) as i64;
            let sy = sy.clamp(0, self.height as i64 - 1) as u32;
            for dx in 0..canvas.width() {
                let sx = (((dx as f64) - off_x) / scale) as i64;
                let sx = sx.clamp(0, self.width as i64 - 1) as u32;
                let index = ((sy * self.width + sx) * 4) as usize;
                let r = self.rgba[index];
                let g = self.rgba[index + 1];
                let b = self.rgba[index + 2];
                let argb = 0xff00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
                canvas.put(dx, dy, argb);
            }
        }
    }
}

#[cfg(test)]
#[path = "wallpaper_tests.rs"]
mod tests;

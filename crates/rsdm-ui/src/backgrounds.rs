//! Stateless animated backdrops for any [`Surface`].

use rsdm_core::domain::{Background, Palette, Rgb};

use crate::design::DEFAULT_SPEED;
use crate::surface::{Cell, Rect, Surface};

/// Matrix rain glyphs - ASCII only, so any font has them.
const MATRIX_CHARS: &[u8] = b"0123456789ABCDEFGHKMNPRSTUVWXYZ#$%*+=<>";
/// Density ramp from empty to solid, drawn from the CP437 block run.
const SHADE: [char; 5] = [' ', '\u{2591}', '\u{2592}', '\u{2593}', '\u{2588}'];

/// Scale the animation tick by `speed` (0..=10) around the natural rate. `0`
/// freezes the animation; `5` is the natural rate; `10` runs at double.
fn scaled_frame(frame: u64, speed: u8) -> u64 {
    (frame as u128 * speed as u128 / DEFAULT_SPEED as u128) as u64
}

/// Paint `kind` across `area` for animation tick `frame`, at `speed` (0..=10).
pub fn draw(
    surface: &mut impl Surface,
    area: Rect,
    kind: Background,
    frame: u64,
    speed: u8,
    p: Palette,
) {
    let frame = scaled_frame(frame, speed);
    match kind {
        Background::None => {}
        Background::Matrix => matrix(surface, area, frame, p),
        Background::Fire => fire(surface, area, frame, p),
        Background::Rain => rain(surface, area, frame, p),
        Background::Plasma => plasma(surface, area, frame, p),
        Background::Starfield => starfield(surface, area, frame, p),
    }
}

fn put(surface: &mut impl Surface, area: Rect, lx: u16, ly: u16, ch: char, color: Rgb) {
    surface.put(area.x + lx, area.y + ly, Cell::new(ch, color));
}

fn matrix(surface: &mut impl Surface, area: Rect, frame: u64, p: Palette) {
    let h = area.h as i32;
    let head_color = lerp(p.accent, Rgb::new(235, 245, 255), 0.7);
    for col in 0..area.w {
        let seed = hash(col as u32 ^ 0x9e37_79b9);
        if seed.is_multiple_of(5) {
            continue;
        }
        let speed = 1 + seed % 3;
        let len = 4 + (seed >> 3) % 11;
        let period = (h as u32 + len + (seed >> 7) % 24).max(1);
        let head = (((frame as u32).wrapping_mul(speed) / 2 + seed) % period) as i32 - len as i32;
        for row in 0..area.h {
            let d = head - row as i32;
            if d < 0 || d as u32 >= len {
                continue;
            }
            let glyph = hash(
                (col as u32)
                    .wrapping_mul(31)
                    .wrapping_add((row as u32).wrapping_mul(7))
                    .wrapping_add((frame / 3) as u32),
            ) as usize
                % MATRIX_CHARS.len();
            let ch = MATRIX_CHARS[glyph] as char;
            let t = d as f32 / len as f32;
            let color = if d == 0 {
                head_color
            } else {
                lerp(p.accent, p.bg_base, t)
            };
            put(surface, area, col, row, ch, color);
        }
    }
}

fn rain(surface: &mut impl Surface, area: Rect, frame: u64, p: Palette) {
    let h = area.h as u32;
    for col in 0..area.w {
        let seed = hash((col as u32).wrapping_mul(2_654_435_761));
        if seed % 5 >= 2 {
            continue; // ~2 of every 5 columns carry a streak
        }
        let speed = 1 + seed % 2;
        let period = (h + 8).max(1);
        let head = ((frame as u32).wrapping_mul(speed) / 2 + seed % period) % period;
        for (offset, (ch, color)) in [('|', p.secondary), ('.', p.fg_muted)]
            .into_iter()
            .enumerate()
        {
            let row = head as i32 - offset as i32;
            if row >= 0 && (row as u32) < h {
                put(surface, area, col, row as u16, ch, color);
            }
        }
    }
}

fn plasma(surface: &mut impl Surface, area: Rect, frame: u64, p: Palette) {
    let t = frame as f32 * 0.13;
    for row in 0..area.h {
        let fy = row as f32;
        for col in 0..area.w {
            let fx = col as f32;
            let v = (fx * 0.08 + t).sin()
                + (fy * 0.11 - t * 0.8).sin()
                + ((fx + fy) * 0.055 + t * 0.6).sin()
                + ((fx * fx + fy * fy).sqrt() * 0.075 - t).sin();
            let n = (v * 0.25) * 0.5 + 0.5; // 0..1
            let hue = if n < 0.5 {
                lerp(p.primary, p.secondary, n * 2.0)
            } else {
                lerp(p.secondary, p.accent, (n - 0.5) * 2.0)
            };
            put(
                surface,
                area,
                col,
                row,
                '\u{2588}',
                lerp(p.bg_base, hue, 0.5),
            );
        }
    }
}

fn fire(surface: &mut impl Surface, area: Rect, frame: u64, p: Palette) {
    let rows = area.h;
    if rows == 0 {
        return;
    }
    let h = rows as f32;
    let white = lerp(p.warning, Rgb::new(255, 250, 240), 0.85);
    let core_rows = (h * 0.22).clamp(6.0, 14.0);
    for row in 0..rows {
        let from_bottom = (rows - 1 - row) as f32; // 0 at the very bottom row
        let frac = from_bottom / h; // 0 at the bottom .. ~1 at the top
        for col in 0..area.w {
            let n = noise(col as i32, row as i32 - (frame / 2) as i32);
            let flame_top = 0.40 + n * 0.10;
            if frac < flame_top {
                let t = 1.0 - frac / flame_top; // 1 at the base .. 0 at the tip
                let heat = (t * 0.85 + n * 0.30).clamp(0.0, 1.0);
                if heat < 0.10 {
                    continue;
                }
                let ch = SHADE[((heat * 4.0) as usize).min(4)];
                let color = if from_bottom < core_rows && heat > 0.80 {
                    white
                } else if heat < 0.45 {
                    lerp(p.bg_base, p.danger, heat / 0.45)
                } else if heat < 0.75 {
                    lerp(p.danger, p.warning, (heat - 0.45) / 0.30)
                } else {
                    lerp(p.warning, white, (heat - 0.75) / 0.25)
                };
                put(surface, area, col, row, ch, color);
            } else {
                let rise = (row as u32).wrapping_add((frame / 3) as u32);
                let seed =
                    hash((col as u32).wrapping_mul(2_654_435_761) ^ rise.wrapping_mul(40_503));
                if !seed.is_multiple_of(40) {
                    continue;
                }
                let g = 80 + (hash(seed ^ (frame / 5) as u32) % 130) as u8;
                let gray = Rgb::new(g, g, g);
                let ch = match seed % 3 {
                    0 => '.',
                    1 => '\'',
                    _ => '*',
                };
                put(surface, area, col, row, ch, gray);
            }
        }
    }
}

fn starfield(surface: &mut impl Surface, area: Rect, frame: u64, p: Palette) {
    for row in 0..area.h {
        for col in 0..area.w {
            let sx = (col as u32).wrapping_add((frame / 2) as u32);
            let seed = hash(sx.wrapping_mul(73_856_093) ^ (row as u32).wrapping_mul(19_349_663));
            if !seed.is_multiple_of(167) {
                continue;
            }
            let tw =
                ((frame as f32 * 0.25 + (seed % 100) as f32).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
            let ch = if tw > 0.72 {
                '*'
            } else if tw > 0.4 {
                '+'
            } else {
                '.'
            };
            put(
                surface,
                area,
                col,
                row,
                ch,
                lerp(p.fg_subtle, p.fg_primary, tw),
            );
        }
    }
}

/// A cheap integer hash for per-column / per-cell pseudo-randomness.
fn hash(mut x: u32) -> u32 {
    x = (x ^ 61) ^ (x >> 16);
    x = x.wrapping_add(x << 3);
    x ^= x >> 4;
    x = x.wrapping_mul(0x27d4_eb2d);
    x ^ (x >> 15)
}

/// Value noise in `0..1` from two hashed lattice samples.
fn noise(x: i32, y: i32) -> f32 {
    let a = hash((x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263));
    let b = hash((x as u32).wrapping_mul(2_246_822_519) ^ (y as u32).wrapping_mul(3_266_489_917));
    ((a ^ b) % 1000) as f32 / 1000.0
}

/// Linear blend `a -> b` by `t` in `0..1`.
fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::testing::VecSurface;
    use rsdm_core::domain::{ThemePreset, theme};

    fn palette() -> Palette {
        theme::palette(ThemePreset::Dracula)
    }

    #[test]
    fn none_draws_nothing() {
        let mut s = VecSurface::new(20, 10);
        let area = Rect::new(0, 0, 20, 10);
        draw(&mut s, area, Background::None, 0, 5, palette());
        assert_eq!(s.dump().trim().len(), 0);
    }

    #[test]
    fn plasma_fills_every_cell() {
        let mut s = VecSurface::new(8, 4);
        draw(
            &mut s,
            Rect::new(0, 0, 8, 4),
            Background::Plasma,
            3,
            5,
            palette(),
        );
        for y in 0..4 {
            for x in 0..8 {
                assert_eq!(s.glyph(x, y), '\u{2588}');
            }
        }
    }

    #[test]
    fn speed_zero_freezes_the_animation() {
        let area = Rect::new(0, 0, 40, 20);
        let mut a = VecSurface::new(40, 20);
        let mut b = VecSurface::new(40, 20);
        draw(&mut a, area, Background::Matrix, 0, 0, palette());
        draw(&mut b, area, Background::Matrix, 999, 0, palette());
        assert_eq!(a.dump(), b.dump(), "speed 0 should not advance the frame");
    }

    #[test]
    fn effects_do_not_panic_at_small_sizes() {
        for kind in [
            Background::Matrix,
            Background::Fire,
            Background::Rain,
            Background::Starfield,
        ] {
            let mut s = VecSurface::new(3, 2);
            draw(&mut s, Rect::new(0, 0, 3, 2), kind, 7, 5, palette());
        }
    }
}

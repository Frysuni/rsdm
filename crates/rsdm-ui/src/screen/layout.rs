//! The shared box, body and HUD layout used by both fronts.
//!
//! The greeter and the locker assemble the same [`BodyLine`] list and hand it to
//! [`draw_framed_box`]; the status and footer go through [`draw_top_hud`] /
//! [`draw_bottom_hud`]. Nothing here knows which front it is drawing for.

use rsdm_core::domain::{Palette, Rgb};

use crate::borders;
use crate::design::Design;
use crate::surface::{Cell, Rect, Surface};
use crate::text::{Role, Segment, draw_segments_centered, segments_width};

use super::{BodyLine, CARET, CONTENT_MAX, CONTENT_MIN, LABEL_WIDTH, VALUE_SLOT};

// --- body assembly --------------------------------------------------------

pub(super) fn push_banner(body: &mut Vec<BodyLine>, title: &[String]) {
    for line in title {
        body.push(BodyLine::Banner(line.clone()));
    }
}

pub(super) fn push_fields(body: &mut Vec<BodyLine>, fields: Vec<(&'static str, String, bool)>) {
    let count = fields.len();
    for (i, (label, value, focused)) in fields.into_iter().enumerate() {
        body.push(BodyLine::Field {
            label,
            value,
            focused,
        });
        if i + 1 < count {
            body.push(BodyLine::Blank);
        }
    }
}

/// Width of the content column, from the widest of banner, fields and error,
/// clamped to a readable range for fields/errors. A wider title can expand the
/// virtual frame beyond the visible surface; the renderer clips it at the edges.
pub(super) fn content_width(
    title: &[String],
    fields: &[(&'static str, String, bool)],
    error: Option<&str>,
    surface_w: u16,
) -> u16 {
    let banner_w = title.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
    let field_w = fields
        .iter()
        .map(|(_, value, _)| field_visible_width(value))
        .max()
        .unwrap_or(0);
    let err_w = error.map(|e| e.chars().count() as u16 + 2).unwrap_or(0);
    let ceil = CONTENT_MAX.min(surface_w.saturating_sub(16));
    let regular_w = field_w
        .max(err_w)
        .max(CONTENT_MIN)
        .min(ceil.max(CONTENT_MIN));
    banner_w.max(regular_w)
}

/// The reserved cell width of a field value (at least [`VALUE_SLOT`], plus one
/// for the caret), so the layout is stable for the first characters typed.
fn field_slot(value: &str) -> usize {
    value.chars().count().max(VALUE_SLOT) + 1
}

fn field_visible_width(value: &str) -> u16 {
    (2 + LABEL_WIDTH + field_slot(value)) as u16
}

// --- framed box -----------------------------------------------------------

pub(super) fn draw_framed_box(
    surface: &mut impl Surface,
    design: &Design,
    area: Rect,
    content_w: u16,
    body: &[BodyLine],
    p: Palette,
    opaque: bool,
) {
    let content_h = body.len() as u16;
    let (fw, fh) = borders::outer_size(design.border, content_w, content_h);
    // Center the box in the region between the status line (row 0) and the
    // footer (3 rows), so they never overlap.
    let region = Rect::new(0, 1, area.w, area.h.saturating_sub(4).max(fh.min(area.h)));
    let frame_h = fh.min(area.h);
    if fw <= area.w {
        let box_area = region.centered(fw, frame_h);
        let interior = borders::draw_frame_with_fill(surface, box_area, design.border, p, opaque);
        draw_body(surface, interior, body, p);
    } else {
        let y = region.y + region.h.saturating_sub(frame_h) / 2;
        let x_offset = (area.w as i32 - fw as i32) / 2;
        let mut shifted = OffsetSurface::new(surface, x_offset, fw);
        let box_area = Rect::new(0, y, fw, frame_h);
        let interior =
            borders::draw_frame_with_fill(&mut shifted, box_area, design.border, p, opaque);
        draw_body(&mut shifted, interior, body, p);
    }
}

fn draw_body(surface: &mut impl Surface, interior: Rect, body: &[BodyLine], p: Palette) {
    let block_w = body
        .iter()
        .filter_map(|line| match line {
            BodyLine::Field { value, .. } => Some(field_visible_width(value)),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let block_left = interior.center_x().saturating_sub(block_w / 2);
    let cx = interior.center_x();
    // The banner art is centered as ONE block: every row shares the same left
    // origin, derived from the widest row. Centering each row by its own trimmed
    // width instead would shift the shorter top/bottom rows sideways and tear the
    // glyph columns apart - the cause of the "first rows drift right" artifact.
    let banner_w = body
        .iter()
        .filter_map(|line| match line {
            BodyLine::Banner(line) => Some(line.chars().count() as u16),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let banner_left = cx.saturating_sub(banner_w / 2);
    for (i, line) in body.iter().enumerate() {
        let y = interior.y + i as u16;
        if y >= interior.bottom() {
            break;
        }
        match line {
            BodyLine::Blank => {}
            BodyLine::Centered(segs) => draw_segments_centered(surface, cx, y, segs, p),
            BodyLine::Banner(line) => draw_banner_line(surface, banner_left, y, line, p.primary),
            BodyLine::Field {
                label,
                value,
                focused,
            } => draw_field(surface, block_left, y, label, value, *focused, p),
        }
    }
}

/// Stamp one row of figlet title art from a shared left origin (`start`). Every
/// glyph shares the same cell pitch, so the art joins seamlessly on the
/// framebuffer. The whole banner block is centered by the caller, so every row
/// lines up column-for-column.
fn draw_banner_line(surface: &mut impl Surface, start: u16, y: u16, line: &str, color: Rgb) {
    for (offset, ch) in line.chars().enumerate() {
        surface.put(
            start.saturating_add(offset as u16),
            y,
            Cell::bold(ch, color),
        );
    }
}

fn draw_field(
    surface: &mut impl Surface,
    x: u16,
    y: u16,
    label: &str,
    value: &str,
    focused: bool,
    p: Palette,
) {
    let (marker, marker_color, label_color) = if focused {
        (">", p.primary, p.primary)
    } else {
        (" ", p.fg_subtle, p.fg_secondary)
    };
    surface.text(x, y, marker, marker_color, focused);
    let label = format!("{label:<LABEL_WIDTH$}");
    surface.text(x + 2, y, &label, label_color, true);
    let value_x = x + 2 + LABEL_WIDTH as u16;
    surface.text(value_x, y, value, p.fg_primary, false);
    // A caret marks where the next character lands on the focused field.
    let caret_x = value_x.saturating_add(value.chars().count() as u16);
    let caret_color = if focused { p.primary } else { p.fg_subtle };
    surface.put(caret_x, y, Cell::text(CARET, caret_color, false));
}

// --- HUD (status / footer) with a background-free margin ------------------

/// Draw a centered, single-row status anchored at the top, after clearing a
/// strictly rectangular margin one cell wider on every side so the animated
/// background never bleeds into it.
pub(super) fn draw_top_hud(
    surface: &mut impl Surface,
    area: Rect,
    segs: &[Segment],
    p: Palette,
    clear_background: bool,
) {
    if segs.is_empty() {
        return;
    }
    let w = segments_width(segs);
    let cx = area.center_x();
    let left = cx.saturating_sub(w / 2);
    let margin = Rect::new(
        left.saturating_sub(1),
        area.y,
        (w + 2).min(area.w),
        2.min(area.h),
    );
    if clear_background {
        clear_rect(surface, margin, p.bg_base);
    }
    draw_segments_centered(surface, cx, area.y, segs, p);
}

/// Draw centered footer lines anchored so the lowest sits at `bottom-2`, after
/// clearing a strictly rectangular margin one cell wider on every side.
pub(super) fn draw_bottom_hud(
    surface: &mut impl Surface,
    area: Rect,
    lines: &[Vec<Segment>],
    p: Palette,
    clear_background: bool,
) {
    if lines.is_empty() {
        return;
    }
    let max_w = lines.iter().map(|l| segments_width(l)).max().unwrap_or(0);
    let cx = area.center_x();
    let left = cx.saturating_sub(max_w / 2);
    let n = lines.len() as u16;
    let bottom_y = area.bottom().saturating_sub(2);
    let top_y = bottom_y.saturating_sub(n - 1);
    let margin = Rect::new(
        left.saturating_sub(1),
        top_y.saturating_sub(1),
        (max_w + 2).min(area.w),
        (n + 2).min(area.h),
    );
    if clear_background {
        clear_rect(surface, margin, p.bg_base);
    }
    for (i, line) in lines.iter().enumerate() {
        draw_segments_centered(surface, cx, top_y + i as u16, line, p);
    }
}

/// Paint a solid rectangle and blank every cell in it, so any animated glyph
/// already drawn there is erased (the TTY keeps glyphs until overwritten).
fn clear_rect(surface: &mut impl Surface, rect: Rect, bg: Rgb) {
    surface.fill(rect, bg);
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            surface.put(x, y, Cell::new(' ', bg));
        }
    }
}

// --- small shared pieces --------------------------------------------------

pub(super) fn too_small(surface: &mut impl Surface, area: Rect, p: Palette) {
    let text = "terminal too small";
    surface.text_centered(area.center_x(), area.center_y(), text, p.danger, true);
}

pub(super) fn key(label: &'static str) -> Segment {
    Segment::bold(label, Role::Primary)
}

pub(super) fn hint(label: &'static str) -> Segment {
    Segment::new(label, Role::FgSecondary)
}

pub(super) fn sep() -> Segment {
    Segment::new("   -   ", Role::FgMuted)
}

pub(super) fn draw_double_frame(surface: &mut impl Surface, r: Rect, color: Rgb) {
    if r.w < 2 || r.h < 2 {
        return;
    }
    let (x0, y0, x1, y1) = (r.x, r.y, r.right() - 1, r.bottom() - 1);
    surface.hrun(x0 + 1, y0, r.w - 2, '\u{2550}', color);
    surface.hrun(x0 + 1, y1, r.w - 2, '\u{2550}', color);
    surface.vrun(x0, y0 + 1, r.h - 2, '\u{2551}', color);
    surface.vrun(x1, y0 + 1, r.h - 2, '\u{2551}', color);
    surface.put(x0, y0, Cell::new('\u{2554}', color));
    surface.put(x1, y0, Cell::new('\u{2557}', color));
    surface.put(x0, y1, Cell::new('\u{255a}', color));
    surface.put(x1, y1, Cell::new('\u{255d}', color));
}

// --- offset surface for an over-wide box ----------------------------------

struct OffsetSurface<'a, S: Surface + ?Sized> {
    inner: &'a mut S,
    x_offset: i32,
    virtual_w: u16,
}

impl<'a, S: Surface + ?Sized> OffsetSurface<'a, S> {
    fn new(inner: &'a mut S, x_offset: i32, virtual_w: u16) -> Self {
        Self {
            inner,
            x_offset,
            virtual_w,
        }
    }

    fn map_x(&self, x: u16) -> Option<u16> {
        let mapped = x as i32 + self.x_offset;
        (0..i32::from(self.inner.cols()))
            .contains(&mapped)
            .then_some(mapped as u16)
    }
}

impl<S: Surface + ?Sized> Surface for OffsetSurface<'_, S> {
    fn size(&self) -> (u16, u16) {
        (self.virtual_w, self.inner.rows())
    }

    fn clear(&mut self, bg: Rgb) {
        self.inner.clear(bg);
    }

    fn fill(&mut self, rect: Rect, bg: Rgb) {
        let x0 = rect.x as i32 + self.x_offset;
        let x1 = x0 + rect.w as i32;
        let visible_x0 = x0.max(0);
        let visible_x1 = x1.min(i32::from(self.inner.cols()));
        if visible_x0 >= visible_x1 {
            return;
        }
        self.inner.fill(
            Rect::new(
                visible_x0 as u16,
                rect.y,
                (visible_x1 - visible_x0) as u16,
                rect.h,
            ),
            bg,
        );
    }

    fn put(&mut self, x: u16, y: u16, cell: Cell) {
        if let Some(mapped) = self.map_x(x) {
            self.inner.put(mapped, y, cell);
        }
    }
}

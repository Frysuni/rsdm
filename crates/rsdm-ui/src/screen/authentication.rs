//! Keep the current authentication response and prompt visible on small screens.

use rsdm_core::domain::Palette;

use crate::{borders, design::Design};
use crate::surface::{Rect, Surface};
use crate::text::{Role, draw_segments_centered};

use super::{BodyLine, CARET, message::push_message};

pub(super) fn fit_body(body: &mut Vec<BodyLine>, design: &Design, area: Rect) {
    let frame = Rect::new(0, 0, area.w, area.h.saturating_sub(4));
    let height = usize::from(borders::interior(design.border, frame).h);
    if body.len() <= height {
        return;
    }

    // Decorations and earlier PAM notices must not hide the response field or
    // the current prompt, which follows the accumulated notices.
    let first_field = body.iter().position(|line| matches!(line, BodyLine::Field { .. }));
    let Some(first_field) = first_field else { return };
    body.drain(..first_field);
    while matches!(body.last(), Some(BodyLine::Blank)) {
        body.pop();
    }
    let last_field = body.iter().rposition(|line| matches!(line, BodyLine::Field { .. })).unwrap();
    if body.len() > height && last_field + 1 < height {
        let omitted = body.len() - height;
        body.drain(last_field + 1..last_field + 1 + omitted);
    }
}

pub(super) fn draw_prompt(
    surface: &mut impl Surface,
    area: Rect,
    value: &str,
    message: Option<&str>,
    message_is_error: bool,
    p: Palette,
) {
    if area.h == 0 {
        return;
    }
    let y = area.center_y().min(area.bottom().saturating_sub(2));
    if y > area.y {
        surface.text_centered(area.center_x(), y - 1, "screen too small", p.warning, true);
    }
    surface.text_centered(area.center_x(), y, &format!("Response: {value}{CARET}"), p.fg_primary, true);
    let Some(message) = message else { return };
    let role = if message_is_error { Role::Danger } else { Role::Info };
    let mut rows = Vec::new();
    push_message(&mut rows, message, role, area.w);
    let available = usize::from(area.bottom().saturating_sub(y + 1));
    for (offset, line) in rows.iter().skip(rows.len().saturating_sub(available)).enumerate() {
        if let BodyLine::Centered(segments) = line {
            draw_segments_centered(surface, area.center_x(), y + 1 + offset as u16, segments, p);
        }
    }
}

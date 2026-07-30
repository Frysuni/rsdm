use rsdm_core::domain::{Palette, Rgb};

use super::{Menu, MenuRow};
use crate::design::Design;
use crate::surface::{Cell, Rect, Surface};
use crate::text::{Role, Segment, draw_segments_centered};

impl Menu {
    pub fn draw(&self, surface: &mut impl Surface, area: Rect, design: &Design, palette: Palette) {
        let view = self.view(design);
        let width = modal_width(area, &view.rows, &view.title, self.notice.as_ref());
        let notice_lines = self
            .notice
            .as_ref()
            .map(|(message, _)| wrap_notice(message, width.saturating_sub(4) as usize, 4))
            .unwrap_or_default();
        let visible = visible_rows(
            view.rows.len(),
            self.index,
            area.h.saturating_sub(notice_lines.len() as u16),
        );
        let height = (visible.len() as u16 + 5 + notice_lines.len() as u16).min(area.h);
        let modal = area.centered(width, height);

        surface.fill(modal, palette.bg_active);
        draw_double_box(surface, modal, palette.accent);
        draw_title(surface, modal, &view.title, palette);
        self.draw_rows(
            surface,
            modal,
            &view.rows,
            visible,
            notice_lines.len(),
            palette,
        );
        self.draw_notice(surface, modal, &notice_lines, palette);
    }

    fn draw_rows(
        &self,
        surface: &mut impl Surface,
        modal: Rect,
        rows: &[MenuRow],
        visible: std::ops::Range<usize>,
        notice_rows: usize,
        palette: Palette,
    ) {
        let visible_rows = &rows[visible.clone()];
        let notice_y = modal.bottom().saturating_sub(1 + notice_rows as u16);
        let mut y = modal.y + 3;

        for (absolute, row) in visible.zip(visible_rows) {
            draw_row(surface, modal, y, row, palette);
            draw_scroll_indicator(
                surface,
                modal,
                y,
                ScrollState {
                    index: absolute,
                    total: rows.len(),
                    visible: visible_rows.len(),
                    selected: row.selected,
                },
                palette,
            );
            y += 1;
            if y >= notice_y {
                break;
            }
        }
    }

    fn draw_notice(
        &self,
        surface: &mut impl Surface,
        modal: Rect,
        lines: &[String],
        palette: Palette,
    ) {
        let Some((_, is_error)) = &self.notice else {
            return;
        };
        let color = if *is_error {
            palette.warning
        } else {
            palette.primary
        };
        let y = modal.bottom().saturating_sub(1 + lines.len() as u16);
        for (offset, line) in lines.iter().enumerate() {
            surface.text(modal.x + 2, y + offset as u16, line, color, true);
        }
    }
}

fn modal_width(area: Rect, rows: &[MenuRow], title: &str, notice: Option<&(String, bool)>) -> u16 {
    let label_width = rows
        .iter()
        .map(|row| row.label.chars().count() as u16)
        .max()
        .unwrap_or(0)
        .max(title.chars().count() as u16)
        .max(
            notice
                .map(|(message, _)| message.chars().count().min(72) as u16)
                .unwrap_or(0),
        );
    (label_width + 8).min(area.w)
}

fn draw_title(surface: &mut impl Surface, modal: Rect, title: &str, palette: Palette) {
    let title = [Segment::bold(format!(" {title} "), Role::Accent)];
    draw_segments_centered(surface, modal.center_x(), modal.y + 1, &title, palette);
}

fn draw_row(surface: &mut impl Surface, modal: Rect, y: u16, row: &MenuRow, palette: Palette) {
    let marker = if row.selected { '>' } else { ' ' };
    let active = if row.active { '*' } else { ' ' };
    let color = if row.selected {
        palette.accent
    } else {
        palette.fg_secondary
    };
    let x = modal.x + 2;

    surface.put(x, y, Cell::new(marker, palette.accent));
    surface.put(x + 1, y, Cell::new(active, palette.primary));
    surface.text(x + 3, y, &row.label, color, row.selected);
}

fn draw_scroll_indicator(
    surface: &mut impl Surface,
    modal: Rect,
    y: u16,
    scroll: ScrollState,
    palette: Palette,
) {
    if scroll.total <= scroll.visible {
        return;
    }
    let color = if scroll.selected {
        palette.primary
    } else {
        palette.fg_subtle
    };
    let glyph = match (scroll.index > 0, scroll.index + 1 < scroll.total) {
        (true, true) => '|',
        (true, false) => '^',
        (false, true) => 'v',
        (false, false) => ' ',
    };
    surface.put(modal.right().saturating_sub(2), y, Cell::new(glyph, color));
}

#[derive(Clone, Copy)]
struct ScrollState {
    index: usize,
    total: usize,
    visible: usize,
    selected: bool,
}

fn wrap_notice(message: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    let chars: Vec<char> = message.chars().collect();
    let mut lines = Vec::new();
    let mut start = 0;
    while start < chars.len() && lines.len() < max_lines {
        let hard_end = (start + width).min(chars.len());
        let end = line_end(&chars, start, hard_end);
        lines.push(chars[start..end].iter().collect());
        start = skip_whitespace(&chars, end);
    }
    truncate_last_line(&mut lines, start < chars.len(), width);
    lines
}

fn line_end(chars: &[char], start: usize, hard_end: usize) -> usize {
    if hard_end == chars.len() {
        return hard_end;
    }
    chars[start..hard_end]
        .iter()
        .rposition(|ch| ch.is_whitespace())
        .filter(|space| *space > 0)
        .map_or(hard_end, |space| start + space)
}

fn skip_whitespace(chars: &[char], mut index: usize) -> usize {
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    index
}

fn truncate_last_line(lines: &mut [String], truncated: bool, width: usize) {
    if !truncated {
        return;
    }
    let Some(last) = lines.last_mut() else {
        return;
    };
    if width < 3 {
        *last = ".".repeat(width);
        return;
    }
    *last = last.chars().take(width - 3).collect();
    last.push_str("...");
}

fn draw_double_box(surface: &mut impl Surface, rect: Rect, color: Rgb) {
    if rect.w < 2 || rect.h < 2 {
        return;
    }
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right() - 1, rect.bottom() - 1);
    surface.hrun(x0 + 1, y0, rect.w - 2, '\u{2550}', color);
    surface.hrun(x0 + 1, y1, rect.w - 2, '\u{2550}', color);
    surface.vrun(x0, y0 + 1, rect.h - 2, '\u{2551}', color);
    surface.vrun(x1, y0 + 1, rect.h - 2, '\u{2551}', color);
    surface.put(x0, y0, Cell::new('\u{2554}', color));
    surface.put(x1, y0, Cell::new('\u{2557}', color));
    surface.put(x0, y1, Cell::new('\u{255a}', color));
    surface.put(x1, y1, Cell::new('\u{255d}', color));
}

fn visible_rows(total: usize, index: usize, area_height: u16) -> std::ops::Range<usize> {
    if total == 0 {
        return 0..0;
    }
    let max_visible = usize::from(area_height.saturating_sub(5)).clamp(1, total);
    let start = index
        .saturating_sub(max_visible / 2)
        .min(total - max_visible);
    start..start + max_visible
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_wrap_and_truncate_to_the_modal() {
        assert_eq!(wrap_notice("one two three", 7, 2), ["one", "two..."]);
        assert_eq!(wrap_notice("abcdef", 1, 2), ["a", "."]);
        assert!(wrap_notice("message", 0, 4).is_empty());
    }

    #[test]
    fn long_menus_scroll_around_the_active_row() {
        let range = visible_rows(200, 150, 20);
        assert!(range.contains(&150));
        assert!(range.start > 0);
        assert!(range.end < 200);
    }
}

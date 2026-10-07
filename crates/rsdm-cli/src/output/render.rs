use std::{io::{self, IsTerminal, Write}, os::fd::AsRawFd};

use ratatui::{
    backend::IntoCrossterm,
    buffer::Buffer,
    crossterm::{queue, style::{Attribute, SetAttribute, SetForegroundColor, SetBackgroundColor}},
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Padding, Paragraph, Widget, Wrap},
};

use super::{ACCENT, BORDER, MUTED, Report};

pub(super) enum Stream { Stdout, Stderr }

impl Stream {
    pub fn width(&self) -> Option<u16> {
        let (tty, fd) = match self {
            Self::Stdout => (io::stdout().is_terminal(), io::stdout().as_raw_fd()),
            Self::Stderr => (io::stderr().is_terminal(), io::stderr().as_raw_fd()),
        };
        if !decorated(tty, std::env::var_os("NO_COLOR").as_deref(), std::env::var("TERM").ok().as_deref()) {
            return None;
        }

        let mut size = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: fd belongs to a live standard stream and size is writable winsize storage.
        let result = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) };
        let columns = if result == 0 && size.ws_col > 0 { size.ws_col } else { 80 };
        // Leave the final column free to avoid terminal autowrap at the border.
        (columns >= 28).then_some(columns.saturating_sub(1).min(100))
    }

    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Stdout => io::stdout().lock().write_all(bytes),
            Self::Stderr => io::stderr().lock().write_all(bytes),
        }
    }
}

pub(super) fn decorated(tty: bool, no_color: Option<&std::ffi::OsStr>, term: Option<&str>) -> bool {
    tty && no_color.is_none_or(std::ffi::OsStr::is_empty) && term != Some("dumb")
}

pub(super) fn report(report: &Report, width: u16) -> Option<Vec<u8>> {
    let body = Paragraph::new(report.lines.clone()).wrap(Wrap { trim: false });
    let height = u16::try_from(body.line_count(width.saturating_sub(4)).checked_add(4)?).ok()?;
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let title = Line::from(vec![
        Span::styled(" RSDM ", Style::new().fg(ACCENT).bold()),
        Span::styled(" / ", Style::new().fg(BORDER)),
        Span::styled(format!("{} ", report.title), Style::new().fg(report.accent).bold()),
    ]);
    let footer = Line::styled(format!(" CLI · v{} ", env!("CARGO_PKG_VERSION")), Style::new().fg(MUTED)).right_aligned();
    let block = Block::bordered().border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER)).title(title).title_bottom(footer)
        .padding(Padding::new(1, 1, 1, 1));
    let inner = block.inner(area);
    block.render(area, &mut buffer);
    body.render(inner, &mut buffer);
    let mut bytes = Vec::new();
    write_buffer(&buffer, &mut bytes).ok()?;
    Some(bytes)
}

fn write_buffer(buffer: &Buffer, writer: &mut impl Write) -> io::Result<()> {
    writeln!(writer)?;
    for y in buffer.area.y..buffer.area.bottom() {
        let mut x = buffer.area.x;
        let mut style = (Color::Reset, Color::Reset, Modifier::empty());
        while x < buffer.area.right() {
            let cell = &buffer[(x, y)];
            let next = (cell.fg, cell.bg, cell.modifier);
            if next != style {
                queue!(writer, SetAttribute(Attribute::Reset),
                    SetForegroundColor(cell.fg.into_crossterm()), SetBackgroundColor(cell.bg.into_crossterm()))?;
                if cell.modifier.contains(Modifier::BOLD) {
                    queue!(writer, SetAttribute(Attribute::Bold))?;
                }
                style = next;
            }
            write!(writer, "{}", cell.symbol())?;
            // Wide graphemes occupy the following cells; printing their filler would shift the row.
            x = x.saturating_add(u16::try_from(Line::raw(cell.symbol()).width().max(1)).unwrap_or(1));
        }
        queue!(writer, SetAttribute(Attribute::Reset))?;
        writeln!(writer)?;
    }
    writeln!(writer)
}

pub(super) fn fragment(lines: Vec<Line<'static>>, width: u16) -> Option<Vec<u8>> {
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let height = u16::try_from(paragraph.line_count(width)).ok()?;
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    paragraph.render(area, &mut buffer);
    let mut bytes = Vec::new();
    write_buffer(&buffer, &mut bytes).ok()?;
    Some(bytes)
}

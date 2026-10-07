//! One-shot CLI reports: ratatui layout, normal terminal scrollback, no input loop.

mod render;
mod session;
mod usage;

use std::io;

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

pub use usage::usage;
pub use session::{application, session_status};

pub const ACCENT: Color = Color::Rgb(94, 234, 212);
pub const SECONDARY: Color = Color::Rgb(167, 139, 250);
pub const SUCCESS: Color = Color::Rgb(134, 239, 172);
pub const WARNING: Color = Color::Rgb(253, 224, 71);
pub const ERROR: Color = Color::Rgb(251, 113, 133);
pub const MUTED: Color = Color::Rgb(148, 163, 184);
pub const BORDER: Color = Color::Rgb(71, 85, 105);

pub struct Report {
    title: String,
    accent: Color,
    lines: Vec<Line<'static>>,
    plain: String,
}

impl Report {
    pub fn new(title: impl Into<String>, accent: Color) -> Self {
        Self { title: title.into(), accent, lines: Vec::new(), plain: String::new() }
    }

    pub fn field(&mut self, label: &str, value: impl Into<String>, color: Color) {
        let value = value.into();
        self.plain.push_str(&format!("{label}: {value}\n"));
        let mut spans = vec![Span::styled(format!("{label:<13} "), Style::new().fg(MUTED))];
        let (state, detail) = value.split_once(' ').unwrap_or((&value, ""));
        if matches!(state, "enabled" | "disabled" | "active" | "inactive" | "failed") {
            spans.push(badge(state, color));
            spans.push(Span::raw(format!(" {}", safe_text(detail))));
        } else {
            spans.push(Span::styled(safe_text(&value), Style::new().fg(color)));
        }
        self.lines.push(Line::from(spans));
    }

    pub fn section(&mut self, title: impl Into<String>) {
        if !self.lines.is_empty() {
            self.lines.push(Line::default());
        }
        self.lines.push(Line::from(vec![
            Span::styled("── ", Style::new().fg(BORDER)),
            Span::styled(title.into(), Style::new().fg(SECONDARY).bold()),
        ]));
    }

    pub fn message(&mut self, plain: impl Into<String>, lines: Vec<Line<'static>>) {
        self.plain.push_str(&plain.into());
        self.lines.extend(lines);
    }

    pub fn stdout(&self) -> io::Result<()> {
        self.write(render::Stream::Stdout)
    }

    pub fn interactive_stdout(&self) -> io::Result<()> {
        if render::Stream::Stdout.width().is_some() {
            self.stdout()?;
        }
        Ok(())
    }

    pub fn stderr(&self) -> io::Result<()> {
        self.write(render::Stream::Stderr)
    }

    fn write(&self, stream: render::Stream) -> io::Result<()> {
        let bytes = match stream.width() {
            Some(width) => render::report(self, width).unwrap_or_else(|| self.plain.as_bytes().to_vec()),
            None => self.plain.as_bytes().to_vec(),
        };
        stream.write(&bytes)
    }
}

pub fn notice(title: &str, text: impl Into<String>, color: Color) -> Report {
    let mut report = Report::new(title, color);
    let text = text.into();
    let mut lines: Vec<_> = text.lines().map(|line| Line::raw(safe_text(line))).collect();
    if let Some(first) = lines.first_mut() {
        let symbol = match color { SUCCESS => "✓", WARNING => "!", ERROR => "×", _ => "◆" };
        first.spans.insert(0, Span::styled(format!("{symbol}  "), Style::new().fg(color).bold()));
    }
    report.message(text, lines);
    report
}

pub fn stderr_is_decorated() -> bool {
    render::Stream::Stderr.width().is_some()
}

pub fn state_color(state: &str) -> Color {
    match state {
        "enabled" | "active" | "running" | "registered" | "closed" | "accepted" | "completed" => SUCCESS,
        "starting" | "preparing" | "stopping_session" | "quitting" | "delegated" | "cancelled" => WARNING,
        "failed" | "invalid" => ERROR,
        _ => MUTED,
    }
}

pub fn badge(text: &str, color: Color) -> Span<'static> {
    Span::styled(format!(" {} ", safe_text(text)), Style::new().fg(Color::Black).bg(color).bold())
}

fn safe_text(text: &str) -> String {
    let mut safe = String::new();
    for ch in text.chars() {
        if ch.is_control() {
            safe.extend(ch.escape_default());
        } else {
            safe.push(ch);
        }
    }
    safe
}

#[cfg(test)]
mod tests;

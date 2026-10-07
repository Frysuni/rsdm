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

pub const ACCENT: Color = Color::Cyan;
pub const SUCCESS: Color = Color::Green;
pub const WARNING: Color = Color::Yellow;
pub const ERROR: Color = Color::Red;
pub const MUTED: Color = Color::DarkGray;

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
        self.lines.push(Line::from(vec![
            Span::styled(format!("{label:<13} "), Style::new().fg(MUTED)),
            Span::styled(safe_text(&value), Style::new().fg(color)),
        ]));
    }

    pub fn section(&mut self, title: impl Into<String>) {
        if !self.lines.is_empty() {
            self.lines.push(Line::default());
        }
        self.lines.push(Line::styled(title.into(), Style::new().fg(self.accent).bold()));
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
    let lines = text.lines().map(|line| Line::raw(safe_text(line))).collect();
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

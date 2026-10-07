use std::process::ExitCode;

use clap::error::ErrorKind;
use ratatui::{style::Style, text::{Line, Span}};

use super::{ACCENT, ERROR, MUTED, Report, SUCCESS, WARNING, render::Stream, safe_text};

pub fn usage(error: clap::Error) -> ExitCode {
    let kind = error.kind();
    let (stream, title, accent) = match kind {
        ErrorKind::DisplayHelp => (Stream::Stdout, "COMMAND GUIDE", ACCENT),
        ErrorKind::DisplayVersion => (Stream::Stdout, "VERSION", ACCENT),
        _ => (Stream::Stderr, "COMMAND ERROR", ERROR),
    };
    let plain = error.render().to_string();
    if stream.width().is_none() {
        let _ = stream.write(plain.as_bytes());
    } else {
        let lines = plain.lines().map(help_line).collect();
        let mut report = Report::new(title, accent);
        report.message(plain, lines);
        let _ = report.write(stream);
    }
    ExitCode::from(error.exit_code() as u8)
}

fn help_line(line: &str) -> Line<'static> {
    let line = safe_text(line);
    if let Some(usage) = line.strip_prefix("Usage: ") {
        return Line::from(vec![
            Span::styled("Usage: ", Style::new().fg(ACCENT).bold()),
            Span::styled(usage.to_string(), Style::new().bold()),
        ]);
    }
    if line.ends_with(':') && !line.starts_with(' ') {
        return Line::styled(line, Style::new().fg(ACCENT).bold());
    }
    if line.starts_with("error:") {
        return Line::styled(line, Style::new().fg(ERROR).bold());
    }
    if line.trim_start().starts_with("tip:") {
        return Line::styled(line, Style::new().fg(WARNING));
    }
    if line.starts_with("  ") {
        if let Some((command, description)) = line.trim_start().split_once("  ") {
            return Line::from(vec![
                Span::raw("  "),
                Span::styled(command.to_string(), Style::new().fg(SUCCESS).bold()),
                Span::styled(format!("  {description}"), Style::new().fg(MUTED)),
            ]);
        }
    }
    Line::raw(line)
}

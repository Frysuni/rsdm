//! Journal records share the CLI palette and render incrementally in scrollback.

use std::io;

use ratatui::{style::{Color, Style}, text::{Line, Span}};
use serde_json::Value;
use time::{OffsetDateTime, macros::format_description};

use super::{ACCENT, BORDER, ERROR, MUTED, SECONDARY, WARNING, badge, render, safe_text};

pub fn journal_record(record: &str) -> io::Result<()> {
    let stream = render::Stream::Stdout;
    let lines = match serde_json::from_str::<Value>(record) {
        Ok(entry) if entry.is_object() => entry_lines(&entry),
        _ => vec![Line::styled(safe_text(record), Style::new().fg(MUTED))],
    };
    let plain = lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n") + "\n";
    let bytes = stream.width().and_then(|width| render::fragment(lines, width))
        .unwrap_or_else(|| plain.into_bytes());
    stream.write(&bytes)
}

fn entry_lines(entry: &Value) -> Vec<Line<'static>> {
    let (level, color) = priority(&field(&entry["PRIORITY"]));
    let source = ["_SYSTEMD_USER_UNIT", "_SYSTEMD_UNIT", "SYSLOG_IDENTIFIER", "_COMM"]
        .iter().map(|key| field(&entry[*key])).find(|value| !value.is_empty()).unwrap_or_else(|| "journal".into());
    event_lines(&timestamp(&field(&entry["__REALTIME_TIMESTAMP"])), level, &source, &field(&entry["MESSAGE"]), color)
}

pub(super) fn event_lines(timestamp: &str, level: &str, source: &str, message: &str, color: Color) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{}  ", safe_text(timestamp)), Style::new().fg(MUTED)),
        badge(level, color),
        Span::styled(format!("  {}", safe_text(source)), Style::new().fg(SECONDARY).bold()),
    ])];
    for line in message.split('\n') {
        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::new().fg(BORDER)),
            Span::raw(safe_text(line)),
        ]));
    }
    lines
}

fn priority(value: &str) -> (&'static str, Color) {
    match value {
        "0" => ("EMERG", ERROR), "1" => ("ALERT", ERROR), "2" => ("CRIT", ERROR),
        "3" => ("ERROR", ERROR), "4" => ("WARN", WARNING), "5" => ("NOTICE", SECONDARY),
        "6" => ("INFO", ACCENT), "7" => ("DEBUG", MUTED), _ => ("LOG", MUTED),
    }
}

fn field(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Array(values) => {
            let bytes: Option<Vec<u8>> = values.iter().map(|value| u8::try_from(value.as_u64()?).ok()).collect();
            match bytes {
                Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                None => values.iter().map(field).collect::<Vec<_>>().join("\n"),
            }
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn timestamp(microseconds: &str) -> String {
    let stamp = microseconds.parse::<u64>().ok()
        .and_then(|value| OffsetDateTime::from_unix_timestamp_nanos(i128::from(value) * 1000).ok());
    stamp.and_then(|stamp| stamp.format(format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3] UTC")).ok())
        .unwrap_or_else(|| "time unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_and_duplicate_fields_preserve_message_content() {
        assert_eq!(field(&serde_json::json!([208, 159, 209, 128, 208, 184, 208, 178, 208, 181, 209, 130])), "Привет");
        assert_eq!(field(&serde_json::json!(["first", "second"])), "first\nsecond");
        assert_eq!(field(&serde_json::json!([255])), "�");
    }

    #[test]
    fn journal_priority_is_not_inferred_from_message_words() {
        let entry = serde_json::json!({"PRIORITY":"6", "MESSAGE":"previous ERROR cleared", "_SYSTEMD_USER_UNIT":"rsdm-lock.service"});
        let lines = entry_lines(&entry);
        assert!(lines[0].to_string().contains(" INFO "));
        assert!(lines[0].to_string().contains("rsdm-lock.service"));
    }

    #[test]
    fn timestamps_are_microseconds_and_invalid_values_are_visible() {
        assert_eq!(timestamp("0"), "1970-01-01 00:00:00.000 UTC");
        assert_eq!(timestamp("1234567"), "1970-01-01 00:00:01.234 UTC");
        assert_eq!(timestamp("not a timestamp"), "time unavailable");
    }
}

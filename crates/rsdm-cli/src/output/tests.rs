use std::ffi::OsStr;

use ratatui::text::Line;

use super::{ACCENT, Report, render};

fn without_styles(bytes: &[u8]) -> String {
    let text = std::str::from_utf8(bytes).unwrap();
    let mut chars = text.chars();
    let mut plain = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            assert_eq!(chars.next(), Some('['));
            loop {
                match chars.next().expect("terminated SGR") {
                    '0'..='9' | ';' => {}
                    'm' => break,
                    other => panic!("CLI must only emit SGR, found {other:?}"),
                }
            }
        } else {
            plain.push(ch);
        }
    }
    plain
}

#[test]
fn layout_preserves_long_values_and_unicode_at_narrow_widths() {
    let mut report = Report::new("STATUS", ACCENT);
    let value = format!("/home/用户/{}", "long-name-".repeat(15));
    report.field("config", &value, ACCENT);
    for width in [27, 40, 79, 100] {
        let plain = without_styles(&render::report(&report, width).unwrap());
        let rows: Vec<_> = plain.lines().filter(|line| !line.is_empty()).collect();
        assert!(rows.iter().all(|line| Line::raw(*line).width() == usize::from(width)));
        let body = rows[1..rows.len() - 1].iter()
            .map(|line| line.trim_matches('│').trim()).collect::<String>();
        assert!(body.contains(&value), "{plain}");
    }
}

#[test]
fn report_cannot_emit_control_sequences_from_values() {
    let mut report = Report::new("STATUS", ACCENT);
    report.field("display", "name\x1b[2J\rchanged\tvalue", ACCENT);
    let plain = without_styles(&render::report(&report, 79).unwrap());
    assert!(plain.contains("name\\u{1b}[2J\\rchanged\\tvalue"));
}

#[test]
fn redirects_no_color_and_dumb_terminals_use_plain_output() {
    assert!(render::decorated(true, None, Some("xterm-256color")));
    assert!(render::decorated(true, Some(OsStr::new("")), Some("xterm")));
    assert!(!render::decorated(false, None, Some("xterm")));
    assert!(!render::decorated(true, Some(OsStr::new("1")), Some("xterm")));
    assert!(!render::decorated(true, None, Some("dumb")));
}

#[test]
fn decorative_sections_do_not_change_plain_fields() {
    let mut report = Report::new("STATUS", ACCENT);
    report.section("COMPONENTS");
    report.field("dm", "enabled", ACCENT);
    report.field("lock", "disabled", ACCENT);
    assert_eq!(report.plain, "dm: enabled\nlock: disabled\n");
}

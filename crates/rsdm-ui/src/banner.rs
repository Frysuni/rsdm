//! Generated title art and status-line text.

use std::{
    fs,
    sync::{Mutex, OnceLock},
};

use figrs::{Figlet, FigletOptions};
use rsdm_core::domain::{BorderStyle, DEFAULT_TITLE_FONT, TitleSource, logo};
use time::{OffsetDateTime, format_description::FormatItem, macros::format_description};

use crate::design::{Design, normalize_title_font};

/// The banner lines for the active [`Design`].
///
/// `session_name` is the currently selected session's display name when the
/// front knows it (the greeter does, the locker does not). It only matters for
/// [`TitleSource::SessionName`]; pass `None` to fall back to the hostname.
pub fn title_lines(design: &Design, session_name: Option<&str>) -> Vec<String> {
    let text = resolve_text(design, session_name);
    let key = TitleCacheKey {
        source: design.title_source,
        text: text.clone(),
        preset: design.title_preset,
        font: design.title_font.clone(),
    };
    let cache = TITLE_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut cached) = cache.lock() {
        if let Some((cached_key, lines)) = cached.as_ref()
            && *cached_key == key
        {
            return lines.clone();
        }
        let lines = build_title_lines(design, &text);
        *cached = Some((key, lines.clone()));
        return lines;
    }
    build_title_lines(design, &text)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TitleCacheKey {
    source: TitleSource,
    text: String,
    preset: rsdm_core::domain::LogoPreset,
    font: String,
}

static TITLE_CACHE: OnceLock<Mutex<Option<(TitleCacheKey, Vec<String>)>>> = OnceLock::new();

fn build_title_lines(design: &Design, text: &str) -> Vec<String> {
    if design.title_source == TitleSource::PresetLogo {
        logo::logo(design.title_preset, text)
    } else {
        art(text, &design.title_font)
    }
}

/// The plain (single-line) title text for the active design.
pub fn resolve_text(design: &Design, session_name: Option<&str>) -> String {
    match design.title_source {
        TitleSource::Custom | TitleSource::PresetLogo => design
            .title_text
            .clone()
            .unwrap_or_else(|| "RSDM".to_string()),
        TitleSource::Hostname => hostname(),
        TitleSource::OsRelease => os_release_name().unwrap_or_else(|| "Linux".to_string()),
        TitleSource::SessionName => session_name.map(str::to_string).unwrap_or_else(hostname),
    }
}

// --- generated ASCII art --------------------------------------------------

/// Render `text` as a FIGlet banner in `font`, exactly as figrs draws it.
pub fn art(text: &str, font: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![text.to_string()];
    }
    let font = normalize_title_font(font);
    let options = FigletOptions {
        font,
        ..FigletOptions::default()
    };
    let generated = Figlet::text(text.to_string(), options)
        .or_else(|_| {
            Figlet::text(
                text.to_string(),
                FigletOptions {
                    font: DEFAULT_TITLE_FONT.to_string(),
                    ..FigletOptions::default()
                },
            )
        })
        .map(|figlet| figlet.text)
        .unwrap_or_else(|_| text.to_string());
    clean_art_lines(&generated)
}

fn clean_art_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        vec![text.trim_end().to_string()]
    } else {
        lines
    }
}

// --- captions and status --------------------------------------------------

/// Caption above the fields, shared by both fronts. With no border it is plain
/// text; otherwise the label sits inside a full-width box-drawing rule.
pub fn caption(label: &str, width: usize, border: BorderStyle) -> String {
    if border == BorderStyle::None {
        return label.to_string();
    }

    const DASH: char = '\u{2500}';
    // Right tee `-|` on the left of the label, left tee `|-` on the right, so
    // the horizontal rule joins each separator cleanly.
    let core = format!("\u{2524} {label} \u{251c}");
    let core_len = core.chars().count();
    if width > core_len + 2 {
        let fill = width - core_len;
        let left = fill / 2;
        format!(
            "{}{core}{}",
            DASH.to_string().repeat(left),
            DASH.to_string().repeat(fill - left)
        )
    } else {
        core
    }
}

/// The machine's hostname, or `localhost` when it cannot be read.
pub fn hostname() -> String {
    static HOSTNAME: OnceLock<String> = OnceLock::new();
    HOSTNAME
        .get_or_init(|| {
            fs::read_to_string("/proc/sys/kernel/hostname")
                .map(|name| name.trim().to_string())
                .ok()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "localhost".to_string())
        })
        .clone()
}

/// `PRETTY_NAME`/`NAME` from `/etc/os-release`, if present.
pub fn os_release_name() -> Option<String> {
    static OS_RELEASE: OnceLock<Option<String>> = OnceLock::new();
    OS_RELEASE
        .get_or_init(|| {
            ["/etc/os-release", "/usr/lib/os-release"]
                .iter()
                .find_map(|path| {
                    let text = fs::read_to_string(path).ok()?;
                    ["PRETTY_NAME", "NAME"]
                        .iter()
                        .find_map(|key| find_os_release_value(&text, key))
                })
        })
        .clone()
}

fn find_os_release_value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    text.lines().find_map(|line| {
        let value = line.strip_prefix(&prefix)?.trim();
        parse_os_release_value(value)
    })
}

fn parse_os_release_value(value: &str) -> Option<String> {
    let Some(first) = value.chars().next() else {
        return Some(String::new());
    };
    if matches!(first, '\'' | '"') {
        if value.chars().last()? != first {
            return None;
        }
        let inner = &value[first.len_utf8()..value.len() - first.len_utf8()];
        if first == '\'' {
            return Some(inner.to_string());
        }
        let mut parsed = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(character) = chars.next() {
            if character == '\\' {
                parsed.push(chars.next()?);
            } else {
                parsed.push(character);
            }
        }
        return Some(parsed);
    }
    Some(value.to_string())
}

/// The wall clock, formatted for the status line. ASCII only.
pub fn clock_text() -> String {
    const FORMAT: &[FormatItem<'_>] = format_description!("[year]-[month]-[day] [hour]:[minute]");
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    now.format(FORMAT)
        .unwrap_or_else(|_| "time unavailable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsdm_core::domain::{DesignConfig, TitleSource};

    #[test]
    fn framed_caption_is_exact_width_and_centered() {
        let line = caption("LOGIN", 30, BorderStyle::Classic);
        assert_eq!(line.chars().count(), 30);
        assert!(line.contains("\u{2524} LOGIN \u{251c}"));
    }

    #[test]
    fn framed_caption_falls_back_when_too_narrow() {
        let line = caption("LOGIN", 4, BorderStyle::Classic);
        assert_eq!(line, "\u{2524} LOGIN \u{251c}");
    }

    #[test]
    fn borderless_caption_has_no_rule() {
        assert_eq!(caption("LOGIN", 30, BorderStyle::None), "LOGIN");
    }

    #[test]
    fn art_from_figrs_is_multiline_and_nonblank() {
        let lines = art("Hi", "ANSI Shadow");
        assert!(lines.len() > 1);
        assert!(lines.iter().any(|l| !l.trim().is_empty()));
    }

    #[test]
    fn unknown_font_falls_back_to_default_figrs_font() {
        let lines = art("Hi", "not a real font");
        assert!(lines.len() > 1);
    }

    #[test]
    fn title_lines_generate_art_from_text_by_default() {
        // Default source is Custom -> generated art from the title text.
        let design = Design::from_config(&DesignConfig::default());
        let lines = title_lines(&design, None);
        assert!(lines.len() > 1, "generated art should be multi-line");
    }

    #[test]
    fn preset_logo_source_uses_the_logo_art() {
        let config = DesignConfig {
            title_mode: TitleSource::PresetLogo,
            ..Default::default()
        };
        let design = Design::from_config(&config);
        let lines = title_lines(&design, None);
        // The RSDM preset logo contains box/underscore art.
        assert!(lines.iter().any(|l| l.contains('_')));
    }

    #[test]
    fn os_release_values_support_quotes_and_escapes() {
        assert_eq!(parse_os_release_value("'single quoted'"), Some("single quoted".into()));
        assert_eq!(
            parse_os_release_value(r#""quoted \"name\"""#),
            Some("quoted \"name\"".into())
        );
        assert_eq!(parse_os_release_value("plain"), Some("plain".into()));
    }

    #[test]
    fn title_cache_reuses_lines_for_the_same_design() {
        let design = Design::from_config(&DesignConfig::default());
        let first = title_lines(&design, None);
        let second = title_lines(&design, None);
        assert_eq!(first, second);
    }
}

use rsdm_core::domain::Session;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub name: Option<String>,
    pub comment: Option<String>,
    pub exec: Option<String>,
    pub desktop_names: Vec<String>,
    pub hidden: bool,
    pub no_display: bool,
    pub try_exec: Option<String>,
    pub entry_type: Option<String>,
}

impl DesktopEntry {
    pub fn to_session(&self, id: &str, source_path: &str) -> Option<Session> {
        if self.is_hidden() || !self.is_application() {
            return None;
        }

        let name = self.name.as_ref()?.trim();
        let exec = self.exec.as_ref()?.trim();
        if name.is_empty() || exec.is_empty() {
            return None;
        }
        let exec = clean_exec(exec);
        if exec.is_empty() {
            return None;
        }

        Some(Session {
            id: id.to_string(),
            name: name.to_string(),
            comment: self.comment.clone(),
            exec,
            desktop_names: self.desktop_names.clone(),
            source_path: source_path.to_string(),
        })
    }

    fn is_hidden(&self) -> bool {
        self.hidden || self.no_display
    }

    fn is_application(&self) -> bool {
        self.entry_type
            .as_deref()
            .is_none_or(|entry_type| entry_type == "Application")
    }
}

pub fn parse_desktop_entry(input: &str) -> Result<DesktopEntry, DesktopEntryError> {
    let mut builder = DesktopEntryBuilder::default();
    let mut in_desktop_entry = false;

    for (index, raw_line) in input.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = &line[1..line.len() - 1] == "Desktop Entry";
            continue;
        }
        if !in_desktop_entry {
            continue;
        }

        let Some((raw_key, raw_value)) = line.split_once('=') else {
            return Err(DesktopEntryError::InvalidLine { line: index + 1 });
        };
        builder.set(raw_key.trim(), raw_value.trim(), index + 1)?;
    }

    Ok(builder.finish())
}

fn clean_exec(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }

        match chars.next() {
            Some('%') => output.push('%'),
            Some('f' | 'F' | 'u' | 'U' | 'i' | 'c' | 'k' | 'v' | 'm') => {}
            Some(other) => {
                output.push('%');
                output.push(other);
            }
            None => output.push('%'),
        }
    }
    output.trim().to_string()
}

#[derive(Debug, Default)]
struct DesktopEntryBuilder {
    name: Option<String>,
    localized_name_fallback: Option<String>,
    exec: Option<String>,
    comment: Option<String>,
    desktop_names: Vec<String>,
    hidden: bool,
    no_display: bool,
    try_exec: Option<String>,
    entry_type: Option<String>,
}

impl DesktopEntryBuilder {
    fn set(&mut self, key: &str, value: &str, line: usize) -> Result<(), DesktopEntryError> {
        match base_key(key) {
            "Name" if key == "Name" => self.name = Some(unescape(value, line)?),
            "Name" if self.localized_name_fallback.is_none() => {
                self.localized_name_fallback = Some(unescape(value, line)?);
            }
            "Exec" => self.exec = Some(unescape(value, line)?),
            "Comment" if key == "Comment" => self.comment = Some(unescape(value, line)?),
            "TryExec" => self.try_exec = Some(unescape(value, line)?),
            "DesktopNames" => self.desktop_names = parse_list(value, line)?,
            "Hidden" => self.hidden = parse_bool(value),
            "NoDisplay" => self.no_display = parse_bool(value),
            "Type" => self.entry_type = Some(unescape(value, line)?),
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> DesktopEntry {
        DesktopEntry {
            name: self.name.or(self.localized_name_fallback),
            comment: self.comment,
            exec: self.exec,
            desktop_names: self.desktop_names,
            hidden: self.hidden,
            no_display: self.no_display,
            try_exec: self.try_exec,
            entry_type: self.entry_type,
        }
    }
}

fn base_key(key: &str) -> &str {
    key.split_once('[').map_or(key, |(base, _)| base)
}

fn parse_bool(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
}

fn parse_list(value: &str, line: usize) -> Result<Vec<String>, DesktopEntryError> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut escaped = false;

    for ch in value.chars() {
        if escaped {
            current.push(unescape_char(ch));
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == ';' {
            push_list_item(&mut items, &mut current);
        } else {
            current.push(ch);
        }
    }

    if escaped {
        return Err(DesktopEntryError::DanglingEscape { line });
    }
    push_list_item(&mut items, &mut current);
    Ok(items)
}

fn push_list_item(items: &mut Vec<String>, current: &mut String) {
    let item = current.trim();
    if !item.is_empty() {
        items.push(item.to_string());
    }
    current.clear();
}

fn unescape(value: &str, line: usize) -> Result<String, DesktopEntryError> {
    let mut output = String::with_capacity(value.len());
    let mut escaped = false;

    for ch in value.chars() {
        if escaped {
            output.push(unescape_char(ch));
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            output.push(ch);
        }
    }

    if escaped {
        return Err(DesktopEntryError::DanglingEscape { line });
    }
    Ok(output)
}

fn unescape_char(ch: char) -> char {
    match ch {
        's' => ' ',
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        other => other,
    }
}

#[derive(Debug, Error)]
pub enum DesktopEntryError {
    #[error("invalid desktop entry line {line}")]
    InvalidLine { line: usize },
    #[error("dangling escape in desktop entry line {line}")]
    DanglingEscape { line: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_wayland_session_entry() {
        let text = "[Desktop Entry]\n\
            Name=Niri\n\
            Comment=A scrollable-tiling compositor\n\
            Exec=niri-session\n\
            Type=Application\n\
            DesktopNames=niri\n";
        let entry = parse_desktop_entry(text).unwrap();
        let session = entry
            .to_session("niri", "/usr/share/wayland-sessions/niri.desktop")
            .unwrap();
        assert_eq!(session.name, "Niri");
        assert_eq!(session.exec, "niri-session");
        assert_eq!(session.desktop_names, vec!["niri".to_string()]);
    }

    #[test]
    fn strips_exec_field_codes() {
        let text = "[Desktop Entry]\nName=X\nExec=foo %U --bar %f\nType=Application\n";
        let session = parse_desktop_entry(text)
            .unwrap()
            .to_session("x", "x")
            .unwrap();
        // Field codes are removed; the residual whitespace is harmless because
        // the argv splitter collapses runs of spaces when launching.
        assert_eq!(session.exec, "foo  --bar");
    }

    #[test]
    fn hidden_entries_are_dropped() {
        let text = "[Desktop Entry]\nName=X\nExec=foo\nType=Application\nHidden=true\n";
        assert!(
            parse_desktop_entry(text)
                .unwrap()
                .to_session("x", "x")
                .is_none()
        );
    }

    #[test]
    fn non_application_entries_are_dropped() {
        let text = "[Desktop Entry]\nName=X\nExec=foo\nType=Link\n";
        assert!(
            parse_desktop_entry(text)
                .unwrap()
                .to_session("x", "x")
                .is_none()
        );
    }
}

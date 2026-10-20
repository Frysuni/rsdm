//! Read systemd EnvironmentFile values without shell execution or expansion.

use std::{collections::BTreeMap, io};

use super::getty_expansion::valid_name;

pub(super) fn parse(text: &str) -> io::Result<BTreeMap<String, String>> {
    if text.chars().any(|ch| ch == '\0' || ch == '\u{feff}' || ('\u{fdd0}'..='\u{fdef}').contains(&ch)
        || ch as u32 & 0xffff >= 0xfffe)
    {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid environment file character"));
    }
    let mut environment = BTreeMap::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        remaining = remaining.trim_start_matches(|ch| matches!(ch, ' ' | '\t' | '\r' | '\n'));
        let end = remaining.find(['\n', '\r']).unwrap_or(remaining.len());
        let line = &remaining[..end];
        if line.starts_with(['#', ';']) || !line.contains('=') {
            remaining = remaining.get(end + 1..).unwrap_or("");
            continue;
        }
        let equals = line.find('=').unwrap();
        let name = line[..equals].trim_end_matches([' ', '\t', '\r']);
        let (value, rest) = read_value(&remaining[equals + 1..]);
        if valid_name(name) { environment.insert(name.to_string(), value); }
        remaining = rest;
    }
    Ok(environment)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueState { Before, Raw, SingleQuoted, DoubleQuoted }

fn read_value(text: &str) -> (String, &str) {
    let mut output = String::new();
    let mut state = ValueState::Before;
    let mut significant = 0;
    let mut chars = text.char_indices();
    while let Some((index, ch)) = chars.next() {
        match (state, ch) {
            (ValueState::Before | ValueState::Raw, '\n' | '\r') => {
                if state == ValueState::Raw { output.truncate(significant); }
                return (output, &text[index + 1..]);
            }
            (ValueState::Before, ' ' | '\t') => {}
            (ValueState::Before, '\'') => state = ValueState::SingleQuoted,
            (ValueState::Before, '"') => state = ValueState::DoubleQuoted,
            (ValueState::SingleQuoted, '\'') | (ValueState::DoubleQuoted, '"') => state = ValueState::Before,
            (ValueState::SingleQuoted, ch) => output.push(ch),
            (ValueState::DoubleQuoted, '\\') => {
                if let Some((_, escaped)) = chars.next() { double_escape(&mut output, escaped); }
            }
            (ValueState::DoubleQuoted, ch) => output.push(ch),
            (_, '\\') => {
                if let Some((_, escaped)) = chars.next() && !matches!(escaped, '\n' | '\r') { output.push(escaped); }
                significant = output.len();
                state = ValueState::Raw;
            }
            (_, ch) => {
                output.push(ch);
                if !matches!(ch, ' ' | '\t' | '\r') { significant = output.len(); }
                state = ValueState::Raw;
            }
        }
    }
    if state == ValueState::Raw { output.truncate(significant); }
    (output, "")
}

fn double_escape(output: &mut String, escaped: char) {
    if escaped == '\n' { return; }
    if !matches!(escaped, '\\' | '"' | '$' | '`') { output.push('\\'); }
    output.push(escaped);
}

#[cfg(test)]
#[path = "getty_environment_file_tests.rs"]
mod tests;

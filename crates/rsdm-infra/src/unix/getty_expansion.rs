//! Expand the already-tokenized getty argv using systemd's environment rules.

use std::collections::BTreeMap;

pub(super) fn expand(argv: &[String], environment: &BTreeMap<String, String>) -> Vec<String> {
    let mut expanded = Vec::new();
    for argument in argv {
        if let Some(name) = argument.strip_prefix('$')
            && !name.starts_with(['{', '$'])
        {
            if valid_name(name) && let Some(value) = environment.get(name) {
                expanded.extend(split_words(value));
            }
        } else {
            expanded.push(expand_word(argument, environment));
        }
    }
    expanded
}

fn expand_word(value: &str, environment: &BTreeMap<String, String>) -> String {
    let mut output = String::new();
    let mut remaining = value;
    while let Some(index) = remaining.find('$') {
        output.push_str(&remaining[..index]);
        remaining = &remaining[index + 1..];
        if let Some(literal) = remaining.strip_prefix('$') {
            output.push('$');
            remaining = literal;
        } else if let Some(variable) = remaining.strip_prefix('{')
            && let Some(end) = variable.find('}')
            && !variable[..end].contains(':')
        {
            let name = &variable[..end];
            if valid_name(name) && let Some(value) = environment.get(name) { output.push_str(value); }
            remaining = &variable[end + 1..];
        } else {
            output.push('$');
        }
    }
    output.push_str(remaining);
    output
}

fn split_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => { if let Some(escaped) = chars.next() { word.push(escaped); } started = true; }
            '\'' | '"' if quote.is_none() => { quote = Some(ch); started = true; }
            ch if quote == Some(ch) => quote = None,
            ch if quote.is_none() && whitespace(ch) => {
                if started { words.push(std::mem::take(&mut word)); started = false; }
            }
            ch => { word.push(ch); started = true; }
        }
    }
    if started { words.push(word); }
    words
}

pub(super) fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn whitespace(ch: char) -> bool { matches!(ch, ' ' | '\t' | '\n' | '\r') }

#[cfg(test)]
#[path = "getty_expansion_tests.rs"]
mod tests;

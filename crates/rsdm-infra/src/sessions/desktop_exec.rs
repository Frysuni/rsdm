//! Desktop Exec syntax is decoded before field expansion and launcher encoding.

use std::{iter::Peekable, str::Chars};

pub(super) fn decode(value: &str) -> Option<String> {
    if !value.is_ascii() || value.chars().any(char::is_control) { return None; }
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' { output.push(ch); continue; }
        output.push(match chars.next()? {
            's' => ' ', 'n' => '\n', 't' => '\t', 'r' => '\r', '\\' => '\\',
            _ => return None,
        });
    }
    Some(output)
}

pub(super) fn command(value: &str, name: &str, icon: Option<&str>, source: &str) -> Option<String> {
    let mut args = Vec::new();
    let mut file_codes = 0;
    for token in tokens(value)? {
        if token == "%i" {
            if let Some(icon) = icon.filter(|icon| !icon.is_empty()) {
                args.extend(["--icon".to_string(), icon.to_string()]);
            }
            continue;
        }
        let argument = expand(&token, name, source, &mut file_codes)?;
        if !argument.is_empty() || token.is_empty() { args.push(argument); }
    }
    if args.first().is_none_or(|program| program.is_empty() || program.contains('=')) { return None; }
    if args.iter().any(|argument| argument.contains('\0')) { return None; }
    Some(encode(&args))
}

fn tokens(value: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == ' ' { continue; }
        let argument = if ch == '"' { quoted(&mut chars)? } else { unquoted(ch, &mut chars)? };
        if chars.peek().is_some_and(|ch| *ch != ' ') { return None; }
        args.push(argument);
    }
    Some(args)
}

fn quoted(chars: &mut Peekable<Chars<'_>>) -> Option<String> {
    let mut argument = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(argument),
            '\\' => {
                let escaped = chars.next()?;
                if !matches!(escaped, '"' | '`' | '$' | '\\') { return None; }
                argument.push(escaped);
            }
            '$' | '`' => return None,
            '%' => {
                if chars.next()? != '%' { return None; }
                argument.push_str("%%");
            }
            _ => argument.push(ch),
        }
    }
    None
}

fn unquoted(first: char, chars: &mut Peekable<Chars<'_>>) -> Option<String> {
    let mut argument = String::new();
    let mut current = Some(first);
    while let Some(ch) = current {
        if reserved(ch) { return None; }
        argument.push(ch);
        current = if chars.peek().is_some_and(|ch| *ch != ' ') { chars.next() } else { None };
    }
    Some(argument)
}

fn reserved(ch: char) -> bool {
    ch.is_control() || matches!(ch, ' ' | '"' | '\'' | '\\' | '>' | '<' | '~' | '|' | '&'
        | ';' | '$' | '*' | '?' | '#' | '(' | ')' | '`')
}

fn expand(token: &str, name: &str, source: &str, file_codes: &mut usize) -> Option<String> {
    let mut output = String::new();
    let mut chars = token.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' { output.push(ch); continue; }
        match chars.next()? {
            '%' => output.push('%'),
            'c' => output.push_str(name),
            'k' => output.push_str(source),
            code @ ('f' | 'u' | 'F' | 'U') => {
                if matches!(code, 'F' | 'U') && token != format!("%{code}") { return None; }
                *file_codes += 1;
                if *file_codes > 1 { return None; }
            }
            'd' | 'D' | 'n' | 'N' | 'v' | 'm' => {}
            _ => return None,
        }
    }
    Some(output)
}

fn encode(args: &[String]) -> String {
    // Session.exec carries the existing launcher's syntax. Field replacements
    // must survive its later argv decoding without being split or expanded again.
    args.iter().map(|arg| {
        if !arg.is_empty() && !arg.chars().any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\\')) {
            return arg.clone();
        }
        let mut encoded = String::from("\"");
        for ch in arg.chars() {
            if matches!(ch, '"' | '\\') { encoded.push('\\'); }
            encoded.push(ch);
        }
        encoded.push('"');
        encoded
    }).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
#[path = "desktop_exec_tests.rs"]
mod tests;

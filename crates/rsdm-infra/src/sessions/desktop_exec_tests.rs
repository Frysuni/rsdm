use super::*;
use crate::{sessions::parse_desktop_entry, unix::split_exec};

fn arguments(value: &str) -> Option<Vec<String>> {
    let value = decode(value)?;
    split_exec(&command(&value, "Example Desktop", Some("example-icon"), "/entries/example.desktop")?).ok()
}

#[test]
fn desktop_quoting_and_both_escape_layers_reach_the_launcher_intact() {
    let value = r#"program "two words" "" "single'quote" "one\\\\two" "\\$HOME" "\\`date\\`" "say \\"hello\\"""#;
    assert_eq!(arguments(value).unwrap(), [
        "program", "two words", "", "single'quote", "one\\two", "$HOME", "`date`", "say \"hello\"",
    ]);
    assert_eq!(arguments(r#"program "line\nnext\tend\rstop""#).unwrap(), ["program", "line\nnext\tend\rstop"]);
    assert_eq!(arguments(r#""/path with spaces/program" "a\sb""#).unwrap(), ["/path with spaces/program", "a b"]);
}

#[test]
fn field_codes_expand_once_into_whole_arguments() {
    let name = "Русский %k \\";
    let source = "/entries/a \"quoted\" %c.desktop";
    let icon = "icon with spaces %f";
    let expanded = command("program --name=%c %k %i %% %%c", name, Some(icon), source).unwrap();
    assert_eq!(split_exec(&expanded).unwrap(), ["program", &format!("--name={name}"), source, "--icon", icon, "%", "%c"]);
}

#[test]
fn file_and_deprecated_codes_disappear_without_empty_arguments() {
    for code in ["f", "u", "F", "U"] {
        assert_eq!(arguments(&format!("program %{code} --keep")).unwrap(), ["program", "--keep"]);
    }
    assert_eq!(arguments("program %d %D %n %N %v %m --keep").unwrap(), ["program", "--keep"]);
    assert_eq!(arguments("program --file=%f pre%dpost").unwrap(), ["program", "--file=", "prepost"]);
    for icon in [None, Some("")] {
        assert_eq!(command("program %i --keep", "Name", icon, "").unwrap(), "program --keep");
    }
    assert_eq!(command("program %k --keep", "Name", None, "").unwrap(), "program --keep");
}

#[test]
fn malformed_and_undefined_field_codes_are_rejected() {
    for value in [
        "program %x", "program %", "program --icon=%i", "program before%F", "program %Usuffix",
        "program %f %u", "program %U %F", "program %f%f", r#"program "%c""#, r#"program "prefix%U""#,
    ] {
        assert!(arguments(value).is_none(), "accepted {value:?}");
    }
    assert_eq!(arguments(r#"program "100%%""#).unwrap(), ["program", "100%"]);
}

#[test]
fn malformed_quoting_and_general_escapes_are_rejected() {
    for value in [
        "", r#""" --session"#, r#"program "unfinished"#, r#"program ab"cd""#, r#"program "ab"cd"#,
        r#"program 'shell quote'"#, r#"program "\q""#, r#"program \"#, r#"program "\\q""#,
        r#"program "$HOME""#, r#"program "`date`""#, "program unquoted$", "program a;b",
        "program\targument", "env=value program", "program é", "program\0bad",
    ] {
        assert!(arguments(value).is_none(), "accepted {value:?}");
    }
}

#[test]
fn every_reserved_character_is_literal_when_properly_quoted() {
    for ch in " \t\n\"'\\><~|&;$*?#()`".chars() {
        let escaped = match ch {
            '"' | '$' | '`' => format!("\\\\{ch}"),
            '\\' => "\\\\\\\\".into(),
            '\t' => "\\t".into(), '\n' => "\\n".into(),
            _ => ch.to_string(),
        };
        assert_eq!(arguments(&format!("program \"{escaped}\"")).unwrap(), ["program".to_string(), ch.to_string()]);
    }
}

#[test]
fn launcher_encoding_preserves_replacement_characters_and_empty_arguments() {
    for value in ["", "plain", "white space", "a\tb\nc\r", "\\", "\"", "'", "%c", "$HOME", "日本語", "a\\\"b"] {
        let args = vec!["program".into(), value.to_string(), format!("prefix{value}suffix")];
        assert_eq!(split_exec(&encode(&args)).unwrap(), args);
    }
}

#[test]
fn desktop_entry_expansion_includes_icon_name_and_source_metadata() {
    let entry = parse_desktop_entry("[Desktop Entry]\nName=Desktop %%\nIcon=icon name\nExec=program %c %k %i %U\n").unwrap();
    let session = entry.to_session("example", "/entries/with spaces.desktop").unwrap();
    assert_eq!(split_exec(&session.exec).unwrap(), ["program", "Desktop %%", "/entries/with spaces.desktop", "--icon", "icon name"]);
}

#[test]
fn localized_exec_is_ignored_and_invalid_exec_cannot_become_a_session() {
    let entry = parse_desktop_entry("[Desktop Entry]\nName=X\nExec=program\nExec[ru]=wrong\n").unwrap();
    assert_eq!(entry.to_session("x", "x.desktop").unwrap().exec, "program");
    for value in ["program %x", "program 'not desktop quoting'", "program %f %u"] {
        let entry = parse_desktop_entry(&format!("[Desktop Entry]\nName=X\nExec={value}\n")).unwrap();
        assert!(entry.to_session("x", "x.desktop").is_none());
    }
    assert!(parse_desktop_entry("[Desktop Entry]\nName=X\nExec=program \\q\n").is_err());
}

#[test]
fn nul_in_field_replacements_cannot_reach_a_launchable_session() {
    for (name, icon, source) in [("bad\0name", None, "file.desktop"),
        ("Name", Some("bad\0icon"), "file.desktop"), ("Name", None, "bad\0source")]
    {
        assert!(command("program %c %i %k", name, icon, source).is_none());
    }
}

#[test]
fn explicitly_escaped_name_whitespace_is_preserved_in_expansion() {
    let entry = parse_desktop_entry("[Desktop Entry]\nName=\\sExample\\s\nExec=program %c\n").unwrap();
    let session = entry.to_session("example", "example.desktop").unwrap();
    assert_eq!(split_exec(&session.exec).unwrap(), ["program", " Example "]);
}

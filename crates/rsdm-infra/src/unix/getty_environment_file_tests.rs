use super::*;

#[test]
fn values_preserve_interior_spaces_and_quotes_without_expansion() {
    let environment = parse(" # ignored\n; ignored too\ninvalid line\n NAME = two words  \nLIST=one 'two three'\nVALUE=$HOME\nEMPTY=\n1INVALID=no\nNAME=last\n").unwrap();
    assert_eq!(environment, BTreeMap::from([
        ("NAME".into(), "last".into()), ("LIST".into(), "one 'two three'".into()),
        ("VALUE".into(), "$HOME".into()), ("EMPTY".into(), "".into()),
    ]));
}

#[test]
fn quotes_and_continuation_lines_follow_environment_file_rules() {
    let environment = parse("SINGLE='one\ntwo'\nDOUBLE=\"one\\\ntwo\\$HOME\\q\"\nRAW=one\\\ntwo\\ \\q\nQUOTED=\"  two words  \"\n").unwrap();
    assert_eq!(environment["SINGLE"], "one\ntwo");
    assert_eq!(environment["DOUBLE"], "onetwo$HOME\\q");
    assert_eq!(environment["RAW"], "onetwo q");
    assert_eq!(environment["QUOTED"], "  two words  ");
}

#[test]
fn escaped_trailing_whitespace_is_preserved() {
    assert_eq!(parse("VALUE=two words\\ \n").unwrap()["VALUE"], "two words ");
}

#[test]
fn carriage_returns_separate_assignments() {
    assert_eq!(parse("FIRST=one\rSECOND=two\r\nTHIRD=three\r").unwrap(), BTreeMap::from([
        ("FIRST".into(), "one".into()), ("SECOND".into(), "two".into()), ("THIRD".into(), "three".into()),
    ]));
}

#[test]
fn invalid_file_characters_are_rejected() {
    for ch in ['\0', '\u{feff}', '\u{fdd0}', '\u{ffff}', '\u{10ffff}'] {
        assert!(parse(&format!("NAME={ch}")).is_err());
    }
}

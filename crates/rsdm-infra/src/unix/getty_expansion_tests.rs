use super::*;

fn expanded(arguments: &[&str], values: &[(&str, &str)]) -> Vec<String> {
    let argv = arguments.iter().map(|value| value.to_string()).collect::<Vec<_>>();
    let environment = values.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();
    expand(&argv, &environment)
}

#[test]
fn braced_values_stay_whole_and_are_not_expanded_again() {
    assert_eq!(expanded(
        &["getty", "--option=${VALUE}", "${VALUE}", "$$VALUE", "${EMPTY}", "${MISSING}"],
        &[("VALUE", "two words $HOME Русский"), ("EMPTY", "")],
    ), ["getty", "--option=two words $HOME Русский", "two words $HOME Русский", "$VALUE", "", ""]);
}

#[test]
fn whole_variables_split_quotes_escapes_and_empty_words() {
    assert_eq!(expanded(
        &["getty", "$VALUE", "$EMPTY", "$MISSING"],
        &[("VALUE", "one 'two three' \"\" four\\ five \"six\\ seven\""), ("EMPTY", "")],
    ), ["getty", "one", "two three", "", "four five", "six seven"]);
}

#[test]
fn ordinary_arguments_and_percent_specifiers_are_literal() {
    assert_eq!(expanded(&["getty", "prefix$VALUE", "%I", "%%", "", "$(touch marker)"], &[]),
        ["getty", "prefix$VALUE", "%I", "%%", ""]);
}

#[test]
fn unclosed_quotes_in_a_value_follow_systemds_relaxed_split() {
    assert_eq!(expanded(&["getty", "$VALUE"], &[("VALUE", "one 'two three")]), ["getty", "one", "two three"]);
}

#[test]
fn variable_names_follow_systemd_rules() {
    for name in ["", "1NAME", "HAS-DASH", "NAME/suffix", "РУССКИЙ"] { assert!(!valid_name(name)); }
    assert!(valid_name("_NAME_1"));
}

#[test]
fn shell_default_value_syntax_is_not_interpreted() {
    assert_eq!(expanded(&["getty", "${MISSING:-fallback}", "${VALUE:+alternative}"], &[("VALUE", "set")]),
        ["getty", "${MISSING:-fallback}", "${VALUE:+alternative}"]);
}

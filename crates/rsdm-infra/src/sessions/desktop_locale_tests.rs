use super::*;

#[test]
fn locale_matching_follows_country_modifier_language_fallback_order() {
    assert_eq!(candidates("sr_RS.UTF-8@latin"), ["sr_RS@latin", "sr_RS", "sr@latin", "sr"]);
    assert_eq!(candidates("sr_RS.UTF-8"), ["sr_RS", "sr"]);
    assert_eq!(candidates("sr.UTF-8@latin"), ["sr@latin", "sr"]);
    assert_eq!(candidates("sr"), ["sr"]);
    for locale in ["", "C", "C.UTF-8", "POSIX"] { assert!(candidates(locale).is_empty()); }
}

#[test]
fn name_and_icon_choose_the_same_locale_without_an_arbitrary_fallback() {
    for key in ["Name", "Icon"] {
        for (available, expected) in [
            (vec!["sr_RS@latin", "sr_RS", "sr@latin", "sr"], "sr_RS@latin"),
            (vec!["sr_RS", "sr@latin", "sr"], "sr_RS"),
            (vec!["sr@latin", "sr"], "sr@latin"),
            (vec!["sr"], "sr"),
            (vec!["ru"], "default"),
        ] {
            let mut value = LocalizedValue::default();
            value.set(key, "default".into());
            for locale in available { value.set(&format!("{key}[{locale}]"), locale.into()); }
            assert_eq!(value.resolve(Some("sr_RS.UTF-8@latin")).as_deref(), Some(expected));
        }
        let mut value = LocalizedValue::default();
        value.set(key, "default".into());
        value.set(&format!("{key}[sr@latin]"), "modifier".into());
        assert_eq!(value.resolve(Some("sr")).as_deref(), Some("default"));
    }
}

#[test]
fn localized_keys_require_a_default_and_last_matching_key_wins() {
    let mut missing_default = LocalizedValue::default();
    missing_default.set("Name[ru]", "Русский".into());
    assert!(missing_default.resolve(Some("ru")).is_none());
    let mut value = LocalizedValue::default();
    value.set("Name", "Default".into());
    value.set("Name[ru]", "First".into());
    value.set("Name[ru]", "Last".into());
    assert_eq!(value.resolve(Some("ru")).as_deref(), Some("Last"));
}

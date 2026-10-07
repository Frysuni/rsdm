//! Name and Icon use the same Desktop Entry locale matching order.

#[derive(Debug, Default)]
pub(super) struct LocalizedValue {
    default: Option<String>,
    variants: Vec<(String, String)>,
}

impl LocalizedValue {
    pub fn set(&mut self, key: &str, value: String) {
        if let Some((_, suffix)) = key.split_once('[') {
            if let Some(locale) = suffix.strip_suffix(']') { self.variants.push((locale.into(), value)); }
        } else { self.default = Some(value); }
    }

    pub fn resolve(self, locale: Option<&str>) -> Option<String> {
        let default = self.default?;
        for candidate in candidates(locale.unwrap_or("C")) {
            if let Some((_, value)) = self.variants.iter().rev().find(|(locale, _)| locale == &candidate) {
                return Some(value.clone());
            }
        }
        Some(default)
    }
}

pub(super) fn current_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter()
        .filter_map(|name| std::env::var(name).ok()).find(|value| !value.is_empty())
}

fn candidates(locale: &str) -> Vec<String> {
    let (base, modifier) = locale.split_once('@').map_or((locale, None), |(base, modifier)| (base, Some(modifier)));
    let base = base.split('.').next().unwrap_or(base);
    if matches!(base, "" | "C" | "POSIX") { return Vec::new(); }
    let language = base.split('_').next().unwrap_or(base);
    let mut result = Vec::new();
    if let Some(modifier) = modifier { result.push(format!("{base}@{modifier}")); }
    result.push(base.to_string());
    if language != base {
        if let Some(modifier) = modifier { result.push(format!("{language}@{modifier}")); }
        result.push(language.to_string());
    }
    result
}

#[cfg(test)]
#[path = "desktop_locale_tests.rs"]
mod tests;

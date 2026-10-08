/// Default directories scanned for `wayland-sessions` `.desktop` files, in
/// priority order. Used by `dm.session_dirs`.
pub fn default_session_paths() -> Vec<String> {
    vec![
        "/usr/local/share/wayland-sessions".to_string(),
        "/usr/share/wayland-sessions".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::default_session_paths;

    #[test]
    fn generic_defaults_prefer_local_admin_sessions() {
        assert_eq!(
            default_session_paths(),
            [
                "/usr/local/share/wayland-sessions",
                "/usr/share/wayland-sessions",
            ]
        );
    }
}

/// Default directories scanned for `wayland-sessions` `.desktop` files, in
/// priority order. Used by `dm.session_dirs`.
pub fn default_session_paths() -> Vec<String> {
    vec![
        "/run/current-system/sw/share/wayland-sessions".to_string(),
        "/usr/share/wayland-sessions".to_string(),
        "/usr/local/share/wayland-sessions".to_string(),
    ]
}

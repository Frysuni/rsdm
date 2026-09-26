use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionManagerConfig {
    /// Wrap the launched compositor in a systemd `--user` graphical session
    /// (like uwsm): export the environment to systemd and D-Bus, start
    /// `graphical-session.target` and `xdg-desktop-autostart.target`, run the
    /// compositor and apps as transient units, and tear everything down cleanly
    /// on exit.
    pub enabled: bool,
    /// Extra environment variable names to import into the user manager and
    /// D-Bus once the compositor is up, in addition to the built-in
    /// Wayland/XDG set.
    pub extra_env: Vec<String>,
    /// How long to wait for the compositor to publish its Wayland environment.
    /// On timeout, targets remain inactive until an explicit finalize.
    pub ready_timeout_secs: u64,
}

impl Default for SessionManagerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_env: Vec::new(),
            ready_timeout_secs: 10,
        }
    }
}

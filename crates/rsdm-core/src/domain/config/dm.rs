use serde::{Deserialize, Serialize};

use super::{DesignConfig, FallbackConfig, TtyConfig, default_session_paths};

/// Greeter settings. Everything about the greeter lives here: the virtual
/// terminal it owns, how it picks a session, what it remembers, the fallback
/// login, and - under `design` - its look. The look is a [`DesignConfig`], the
/// same type the locker uses under `lock.design`, so the two fronts are styled
/// independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DmConfig {
    /// Whether the greeter runs at all. When `false`, `rsdm dm` exits without
    /// presenting a login.
    pub enable: bool,
    /// PAM service name; needs a matching stack in `/etc/pam.d/<name>`.
    pub pam_service: String,
    /// The virtual terminal the greeter owns and the seat it belongs to.
    pub tty: TtyConfig,
    /// When the greeter cannot run, hand the terminal to this login program.
    pub fallback: FallbackConfig,
    /// A `.desktop` id, a session Name, or a literal command to always launch,
    /// hiding the session picker. `None` shows the picker over discovered
    /// sessions.
    pub fixed_session: Option<String>,
    /// Directories scanned for `wayland-sessions` `.desktop` files when no fixed
    /// session is set.
    pub session_dirs: Vec<String>,
    /// What the greeter pre-fills on the next launch.
    pub remember: RememberConfig,
    /// The greeter's look.
    pub design: DesignConfig,
}

impl Default for DmConfig {
    fn default() -> Self {
        Self {
            enable: true,
            pam_service: "rsdm".to_string(),
            tty: TtyConfig::default(),
            fallback: FallbackConfig::default(),
            fixed_session: None,
            session_dirs: default_session_paths(),
            remember: RememberConfig::default(),
            design: DesignConfig::default(),
        }
    }
}

impl DmConfig {
    /// Whether the greeter discovers and offers a session picker (no fixed
    /// session configured).
    pub fn uses_picker(&self) -> bool {
        self.fixed_session.as_deref().is_none_or(str::is_empty)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RememberConfig {
    pub username: bool,
    pub session: bool,
}

impl Default for RememberConfig {
    fn default() -> Self {
        Self {
            username: true,
            session: true,
        }
    }
}

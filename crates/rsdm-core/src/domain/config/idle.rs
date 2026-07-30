use serde::{Deserialize, Serialize};

/// Wayland idle daemon settings (`rsdm idle`).
///
/// The compositor remains the source of truth for user activity.  An empty
/// `lock_command` selects rsdm's built-in locker; a non-empty argv lets an
/// operator deliberately substitute another locker without invoking a shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IdleConfig {
    /// Start monitoring idle time.  `rsdm idle` is a no-op when disabled.
    pub enable: bool,
    /// Seconds of compositor-reported inactivity before locking.
    pub timeout: u64,
    /// Ignore idle inhibitors and consider input activity only.
    pub ignore_inhibitors: bool,
    /// Optional locker argv. Empty means the current rsdm binary plus `lock`.
    pub lock_command: Vec<String>,
    /// Shell commands run after the built-in locker is confirmed active.
    pub on_lock: Vec<String>,
    /// Shell commands run after the locker exits, including emergency unlock.
    pub on_unlock: Vec<String>,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            enable: false,
            timeout: 300,
            ignore_inhibitors: false,
            lock_command: Vec::new(),
            on_lock: Vec::new(),
            on_unlock: Vec::new(),
        }
    }
}

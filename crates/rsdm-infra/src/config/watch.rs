//! Validated session policy updates, independent of coordinator lifetime.

use std::{path::{Path, PathBuf}, time::Duration};

use rsdm_core::domain::SessionManagerUpdate;
use tokio::{sync::watch, time::{Instant, MissedTickBehavior}};

use super::{ConfigLoadError, load_config_async};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Read the initial policy and publish later valid changes. Dropping all
/// receivers ends monitoring; replacing the file with rename is supported.
pub async fn watch_session_config(
    path: &Path,
) -> Result<watch::Receiver<SessionManagerUpdate>, ConfigLoadError> {
    let initial = load_config_async(path).await?.session_manager;
    let (sender, receiver) = watch::channel(SessionManagerUpdate::from(initial));
    tokio::spawn(monitor(path.to_path_buf(), sender));
    Ok(receiver)
}

async fn monitor(path: PathBuf, sender: watch::Sender<SessionManagerUpdate>) {
    // Reopen the path instead of watching one inode: editors commonly replace
    // the configuration atomically, and a missing file can later reappear.
    let mut tick = tokio::time::interval_at(Instant::now() + POLL_INTERVAL, POLL_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_error = None;

    loop {
        tokio::select! {
            biased;
            _ = sender.closed() => return,
            _ = tick.tick() => {}
        }
        let result = tokio::select! {
            biased;
            _ = sender.closed() => return,
            result = load_config_async(&path) => result,
        };
        match result {
            Ok(config) => {
                last_error = None;
                sender.send_if_modified(|current| {
                    if current.config == config.session_manager { return false; }
                    if current.config.enabled && !config.session_manager.enabled {
                        current.disable_count += 1;
                    }
                    current.config = config.session_manager;
                    true
                });
            }
            Err(error) => {
                let message = error.to_string();
                if last_error.as_ref() != Some(&message) {
                    tracing::warn!(path = %path.display(), %error, "keeping the previous session configuration");
                    last_error = Some(message);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;

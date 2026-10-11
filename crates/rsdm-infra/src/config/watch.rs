//! Validated session policy updates, independent of coordinator lifetime.

use std::{path::{Path, PathBuf}, time::Duration};

use rsdm_core::domain::SessionManagerConfig;
use tokio::{sync::watch, time::{Instant, MissedTickBehavior}};

use super::{ConfigLoadError, load_config_async};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Read the initial policy and publish later valid changes. Dropping all
/// receivers ends monitoring; replacing the file with rename is supported.
pub async fn watch_session_config(
    path: &Path,
) -> Result<watch::Receiver<SessionManagerConfig>, ConfigLoadError> {
    let initial = load_config_async(path).await?.session_manager;
    let (sender, receiver) = watch::channel(initial);
    tokio::spawn(monitor(path.to_path_buf(), sender));
    Ok(receiver)
}

async fn monitor(path: PathBuf, sender: watch::Sender<SessionManagerConfig>) {
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
                    if *current == config.session_manager { return false; }
                    *current = config.session_manager;
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

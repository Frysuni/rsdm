use super::{
    env::manager_wayland_display,
    systemd::{ANCHOR_UNIT, SESSION_TARGET, anchor_graphical_session, unit_is_active},
};
use rsdm_core::domain::SessionManagerConfig;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

/// Who owns `graphical-session.target` for teardown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionOwnership {
    Anchored,
    External,
    Unmanaged,
}

/// Let a self-managing compositor own the target; otherwise anchor it after its
/// Wayland environment appears.
pub(super) fn wait_and_anchor(
    cfg: &SessionManagerConfig,
    session_unit: &str,
    session_running: &AtomicBool,
    previous_display: Option<String>,
) -> SessionOwnership {
    let deadline = Instant::now() + Duration::from_secs(cfg.ready_timeout_secs);
    let mut published_display = None;
    tracing::debug!(
        timeout_secs = cfg.ready_timeout_secs,
        "waiting for the compositor to become ready"
    );
    loop {
        if !session_running.load(Ordering::SeqCst) {
            tracing::debug!("compositor exited before the graphical session was ready");
            return SessionOwnership::Unmanaged;
        }
        if unit_is_active(SESSION_TARGET) {
            if unit_is_active(ANCHOR_UNIT) {
                return SessionOwnership::Anchored;
            }
            tracing::info!(
                "compositor activated graphical-session.target itself; leaving it to manage the session"
            );
            return SessionOwnership::External;
        }
        if display_ready(
            manager_wayland_display(),
            &previous_display,
            &mut published_display,
            Instant::now(),
        ) {
            tracing::info!("compositor published a new display; anchoring the graphical session");
            return anchor_ownership(session_unit);
        }
        if Instant::now() >= deadline {
            tracing::warn!(
                "compositor did not signal readiness before timeout; leaving graphical targets inactive"
            );
            return SessionOwnership::Unmanaged;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn display_ready(
    current: Option<String>,
    previous: &Option<String>,
    published: &mut Option<(String, Instant)>,
    now: Instant,
) -> bool {
    let Some(display) = current.filter(|display| Some(display) != previous.as_ref()) else {
        *published = None;
        return false;
    };
    if let Some((observed, since)) = published
        && *observed == display
    {
        return now.duration_since(*since) >= Duration::from_secs(2);
    }
    *published = Some((display, now));
    false
}

fn anchor_ownership(session_unit: &str) -> SessionOwnership {
    if anchor_graphical_session(Some(session_unit)) {
        SessionOwnership::Anchored
    } else if unit_is_active(SESSION_TARGET) {
        SessionOwnership::External
    } else {
        SessionOwnership::Unmanaged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_or_disappearing_display_does_not_activate_a_session() {
        let old = Some("WAYLAND_DISPLAY=wayland-0".to_string());
        let new = Some("WAYLAND_DISPLAY=wayland-1".to_string());
        let start = Instant::now();
        let later = start + Duration::from_secs(3);
        let mut published = None;
        assert!(!display_ready(old.clone(), &old, &mut published, start));
        assert!(!display_ready(old.clone(), &old, &mut published, later));
        assert!(!display_ready(new.clone(), &old, &mut published, start));
        assert!(!display_ready(None, &old, &mut published, later));
        assert!(!display_ready(new.clone(), &old, &mut published, later));
        assert!(display_ready(
            new,
            &old,
            &mut published,
            later + Duration::from_secs(2)
        ));
    }
}

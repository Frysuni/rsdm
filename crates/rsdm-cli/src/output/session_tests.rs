use super::*;
use rsdm_session_manager::{Capability, PreparationStatus};

fn details() -> SessionStatusDetails {
    SessionStatusDetails { enabled: true, state: "full".into(), status: SessionStatus {
        generation: "generation".into(), login_session_id: "login".into(), desktop_entry_id: "desktop".into(),
        provider: "managed".into(), phase: "running".into(), xsmp_available: false,
        apps: vec![("app.service".into(), "command".into(), 5, "registered".into())],
    }, capabilities: vec![Capability { name: "window-close".into(), state: "unavailable".into(), reason: "No supported IPC endpoint".into() }],
        limitations: Vec::new(), preparations: Vec::new() }
}

#[test]
fn plain_status_preserves_legacy_prefix_and_reports_optional_capability_failure_as_full() {
    let report = details_report(&details());
    assert!(report.plain.starts_with("managed: running (login login, desktop desktop, generation generation)\n"));
    assert!(report.plain.contains("app.service: registered, command, 5s\n"));
    assert!(report.plain.contains("manager: full\n"));
    assert!(report.plain.contains("capability: window-close: unavailable (No supported IPC endpoint)\n"));
}

#[test]
fn legacy_fallback_preserves_the_complete_old_report() {
    let mut details = details();
    details.capabilities.clear();
    details.limitations.push("Older coordinator".into());
    assert_eq!(details_report(&details).plain, status_report(&details.status).plain);
}

#[test]
fn partial_status_explains_scope_and_escapes_preparation_text() {
    let mut details = details();
    details.state = "partial".into();
    details.limitations.push("External WM and PAM are not owned".into());
    details.preparations.push(PreparationStatus { unit: "app\u{1b}.service".into(), invocation_id: vec![1; 16], transaction_id: "transaction".into(),
        method: "window-close".into(), dispatch_state: "sent".into(), result: "clean".into(), escalated: false });
    let report = details_report(&details);
    assert!(report.plain.contains("manager: partial\n"));
    assert!(report.plain.contains("limitation: External WM and PAM are not owned\n"));
    assert!(report.plain.contains("preparation: app\\u{1b}.service: clean, method window-close, dispatch sent, escalated false\n"));
    assert!(!report.plain.contains('\u{1b}'));
}

#[test]
fn inactive_and_disabled_have_distinct_short_reports() {
    let mut details = details();
    details.state = "inactive".into();
    details.status.phase = "inactive".into();
    details.capabilities.clear();
    assert_eq!(details_report(&details).plain, "inactive: no session manager coordinator is running\n");
    details.enabled = false;
    details.status.phase = "disabled".into();
    assert_eq!(details_report(&details).plain, "disabled: session manager is disabled\n");
}

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use super::*;

struct Fixture { directory: PathBuf, runtime: Runtime, app: AppRecord }

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-dispatch-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::create_at(&directory, "0123456789abcdef0123456789abcdef").unwrap();
        let app = AppRecord { unit: "app-rsdm-dispatch@0123456789abcdef0123456789abcdef-1.service".into(),
            invocation_id: vec![1; 16], policy: Default::default(), deadline_usec: None, quit_started: false,
            dispatch: None, window_close_sent: false, preparation: None };
        runtime.save_app(&app).unwrap();
        Self { directory, runtime, app }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

#[tokio::test]
async fn competing_claims_write_one_intent_before_any_dispatch_is_allowed() {
    let fixture = Fixture::new();
    let deadline = Deadline::default();
    let (first, second) = tokio::join!(claim(&fixture.runtime, &fixture.app, Some("tx"), Method::Command, &deadline),
        claim(&fixture.runtime, &fixture.app, Some("tx"), Method::Command, &deadline));
    assert_ne!(first.unwrap(), second.unwrap());
    let saved = fixture.runtime.app(&fixture.app.unit).unwrap();
    let dispatch = saved.dispatch.unwrap();
    assert_eq!(dispatch.state, State::Intent);
    assert_eq!(dispatch.invocation_id, fixture.app.invocation_id);
    assert_eq!(dispatch.transaction_id.as_deref(), Some("tx"));
    assert!(saved.quit_started);
}

#[tokio::test]
async fn unknown_command_and_window_close_intents_survive_cancel_and_a_later_transaction() {
    for method in [Method::Command, Method::WindowClose] {
        let fixture = Fixture::new();
        assert!(claim(&fixture.runtime, &fixture.app, Some("first"), method, &Deadline::default()).await.unwrap());
        let mut saved = fixture.runtime.app(&fixture.app.unit).unwrap();
        cancel(&mut saved);
        fixture.runtime.save_app(&saved).unwrap();
        assert!(!claim(&fixture.runtime, &fixture.app, Some("next"), method, &Deadline::default()).await.unwrap());
        assert!(!claim(&fixture.runtime, &fixture.app, Some("next"), Method::Term, &Deadline::default()).await.unwrap());
    }
}

#[tokio::test]
async fn only_proven_not_sent_dispatch_can_be_replaced_by_a_fallback() {
    let fixture = Fixture::new();
    assert!(claim(&fixture.runtime, &fixture.app, Some("first"), Method::WindowClose, &Deadline::default()).await.unwrap());
    finish(&fixture.runtime, &fixture.app, Some("first"), Method::WindowClose, State::NotSent, &Deadline::default()).await.unwrap();
    let saved = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert!(!saved.window_close_sent);
    assert!(claim(&fixture.runtime, &fixture.app, Some("first"), Method::Term, &Deadline::default()).await.unwrap());
}

#[tokio::test]
async fn legacy_quit_started_and_a_replacement_invocation_never_allow_an_unsafe_replay() {
    let fixture = Fixture::new();
    let mut legacy = fixture.app.clone();
    legacy.quit_started = true;
    let encoded = toml::to_string(&legacy).unwrap().replace("window_close_sent = false\n", "");
    let legacy: AppRecord = toml::from_str(&encoded).unwrap();
    assert!(legacy.dispatch.is_none());
    assert!(!legacy.window_close_sent);
    fixture.runtime.save_app(&legacy).unwrap();
    assert!(!claim(&fixture.runtime, &fixture.app, Some("new"), Method::WindowClose, &Deadline::default()).await.unwrap());
    let mut replacement = fixture.app.clone();
    replacement.invocation_id = vec![2; 16];
    fixture.runtime.save_app(&replacement).unwrap();
    assert!(claim(&fixture.runtime, &fixture.app, Some("new"), Method::Command, &Deadline::default()).await.is_err());
}

#[tokio::test]
async fn a_confirmed_cancelled_xsmp_save_can_restart_only_in_a_new_transaction() {
    let fixture = Fixture::new();
    assert!(claim(&fixture.runtime, &fixture.app, Some("first"), Method::Xsmp, &Deadline::default()).await.unwrap());
    finish(&fixture.runtime, &fixture.app, Some("first"), Method::Xsmp, State::Sent, &Deadline::default()).await.unwrap();
    let mut saved = fixture.runtime.app(&fixture.app.unit).unwrap();
    cancel(&mut saved);
    fixture.runtime.save_app(&saved).unwrap();
    assert!(!claim(&fixture.runtime, &fixture.app, Some("first"), Method::Xsmp, &Deadline::default()).await.unwrap());
    assert!(claim(&fixture.runtime, &fixture.app, Some("next"), Method::Xsmp, &Deadline::default()).await.unwrap());
    finish(&fixture.runtime, &fixture.app, Some("first"), Method::Xsmp, State::Sent, &Deadline::default()).await.unwrap();
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().dispatch.unwrap().state, State::Intent);
}

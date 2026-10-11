use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use super::*;

struct Fixture { directory: PathBuf, runtime: Runtime, app: AppRecord }

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-result-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::create_at(&directory, crate::helper_start_fixture::GENERATION).unwrap();
        let app = crate::helper_start_fixture::app();
        runtime.save_app(&app).unwrap();
        Self { directory, runtime, app }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

#[tokio::test]
async fn outcomes_are_private_and_survive_removal_of_the_app_and_its_lease() {
    let fixture = Fixture::new();
    let deadline = Deadline::default();
    begin(&fixture.runtime, &fixture.app, Some("tx"), &deadline).await.unwrap();
    assert_eq!(finish(&fixture.runtime, &fixture.app, Some("tx"), Outcome::Clean, &deadline).await.unwrap(), Outcome::Clean);
    fixture.runtime.remove_app(&fixture.app.unit).unwrap();
    fixture.runtime.reap_closed_app_locks().unwrap();
    let archived = fixture.runtime.archived_app(&fixture.app.unit).unwrap();
    assert_eq!(archived.invocation_id, fixture.app.invocation_id);
    assert_eq!(archived.preparation.unwrap().outcome, Outcome::Clean);
    let path = fixture.runtime.path.join("results").join(format!("{}.toml", fixture.app.unit));
    assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(finish(&fixture.runtime, &fixture.app, Some("stale"), Outcome::Cancelled, &deadline).await.unwrap(), Outcome::Unknown);
    assert_eq!(fixture.runtime.archived_app(&fixture.app.unit).unwrap().preparation.unwrap().outcome, Outcome::Clean);
}

#[tokio::test]
async fn an_unsafe_archive_does_not_remove_the_active_record() {
    let fixture = Fixture::new();
    begin(&fixture.runtime, &fixture.app, Some("tx"), &Deadline::default()).await.unwrap();
    std::os::unix::fs::symlink(&fixture.directory, fixture.runtime.path.join("results")).unwrap();
    assert!(fixture.runtime.remove_app(&fixture.app.unit).is_err());
    assert!(fixture.runtime.app(&fixture.app.unit).is_ok());
}

#[tokio::test]
async fn a_saved_escalation_intent_prevents_a_later_recovery_from_claiming_a_clean_exit() {
    let fixture = Fixture::new();
    let deadline = Deadline::default();
    begin(&fixture.runtime, &fixture.app, Some("first"), &deadline).await.unwrap();
    escalating(&fixture.runtime, &fixture.app, Some("first"), &deadline).await.unwrap();
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().preparation.unwrap().outcome, Outcome::Unknown);
    begin(&fixture.runtime, &fixture.app, Some("retry"), &deadline).await.unwrap();
    assert_eq!(finish(&fixture.runtime, &fixture.app, Some("retry"), Outcome::Clean, &deadline).await.unwrap(), Outcome::Forced);
}

#[tokio::test]
async fn legacy_unknown_dispatch_and_unpinned_invocations_never_become_clean() {
    let fixture = Fixture::new();
    for invocation in [vec![1; 16], Vec::new()] {
        let mut app = fixture.app.clone();
        app.invocation_id = invocation;
        app.quit_started = true;
        fixture.runtime.save_app(&app).unwrap();
        begin(&fixture.runtime, &app, None, &Deadline::default()).await.unwrap();
        assert_eq!(finish(&fixture.runtime, &app, None, Outcome::Clean, &Deadline::default()).await.unwrap(), Outcome::Unknown);
    }
}

#[tokio::test]
async fn stale_completions_cannot_replace_a_new_transaction_or_invocation() {
    let fixture = Fixture::new();
    let deadline = Deadline::default();
    begin(&fixture.runtime, &fixture.app, Some("first"), &deadline).await.unwrap();
    begin(&fixture.runtime, &fixture.app, Some("next"), &deadline).await.unwrap();
    assert_eq!(finish(&fixture.runtime, &fixture.app, Some("first"), Outcome::Clean, &deadline).await.unwrap(), Outcome::Unknown);
    let mut replacement = fixture.runtime.app(&fixture.app.unit).unwrap();
    replacement.invocation_id = vec![2; 16];
    fixture.runtime.save_app(&replacement).unwrap();
    assert_eq!(finish(&fixture.runtime, &fixture.app, Some("next"), Outcome::Cancelled, &deadline).await.unwrap(), Outcome::Unknown);
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().preparation.unwrap().outcome, Outcome::Unknown);
}

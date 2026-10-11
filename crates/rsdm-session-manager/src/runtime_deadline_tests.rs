use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use super::*;

struct Fixture { directory: PathBuf, runtime: Runtime }

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-follow-deadline-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::create_at(&directory, crate::helper_start_fixture::GENERATION).unwrap();
        let mut record = crate::helper_start_fixture::record();
        // SAFETY: geteuid has no preconditions; this private record belongs to the test.
        record.identity.uid = unsafe { libc::geteuid() };
        runtime.save_session(&record).unwrap();
        Self { directory, runtime }
    }

    fn save_deadline(&self, deadline: Option<u64>) {
        let mut record = self.runtime.session().unwrap();
        record.shutdown_deadline_usec = deadline;
        self.runtime.save_session(&record).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

fn pending(runtime: Runtime, deadline: Deadline) -> (tokio::task::JoinHandle<Result<(), SessionError>>, tokio::sync::oneshot::Receiver<()>) {
    let (started, ready) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        follow(&runtime, &deadline, async {
            started.send(()).unwrap();
            std::future::pending().await
        }).await
    });
    (worker, ready)
}

#[tokio::test]
async fn a_later_saved_deadline_interrupts_work_started_without_a_budget() {
    let fixture = Fixture::new();
    let deadline = Deadline::default();
    let (worker, ready) = pending(fixture.runtime.clone(), deadline.clone());
    ready.await.unwrap();
    let saved = crate::processes::monotonic_usec().unwrap() + 150_000;
    fixture.save_deadline(Some(saved));
    assert!(tokio::time::timeout(Duration::from_secs(1), worker).await.unwrap().unwrap().is_err());
    assert_eq!(deadline.get(), saved);
}

#[tokio::test]
async fn shortening_the_saved_deadline_wakes_an_already_pending_operation() {
    let fixture = Fixture::new();
    let deadline = Deadline::default();
    deadline.set(crate::processes::monotonic_usec().unwrap() + 30_000_000);
    let (worker, ready) = pending(fixture.runtime.clone(), deadline.clone());
    ready.await.unwrap();
    let saved = crate::processes::monotonic_usec().unwrap() + 150_000;
    fixture.save_deadline(Some(saved));
    assert!(tokio::time::timeout(Duration::from_secs(1), worker).await.unwrap().unwrap().is_err());
    assert_eq!(deadline.get(), saved);
}

#[tokio::test]
async fn replacement_or_revoked_budgets_never_extend_an_accepted_hook() {
    for saved in [None, Some(crate::processes::monotonic_usec().unwrap() + 30_000_000)] {
        let fixture = Fixture::new();
        fixture.save_deadline(saved);
        let deadline = Deadline::default();
        let original = crate::processes::monotonic_usec().unwrap() + 150_000;
        deadline.set(original);
        let (worker, ready) = pending(fixture.runtime.clone(), deadline.clone());
        ready.await.unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(1), worker).await.unwrap().unwrap().is_err());
        assert_eq!(deadline.get(), original);
    }
}

#[tokio::test]
async fn a_foreign_record_cannot_supply_a_hook_budget() {
    let fixture = Fixture::new();
    let mut record = fixture.runtime.session().unwrap();
    record.identity.uid = record.identity.uid.wrapping_add(1);
    fixture.runtime.save_session(&record).unwrap();
    let (worker, _ready) = pending(fixture.runtime.clone(), Deadline::default());
    assert!(matches!(tokio::time::timeout(Duration::from_secs(1), worker).await.unwrap().unwrap(), Err(SessionError::State(_))));
}

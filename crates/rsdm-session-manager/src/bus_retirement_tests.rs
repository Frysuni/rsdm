//! Retirement is checked after the shared IPC queue, not only at hook entry.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use super::*;
use crate::runtime::Runtime;

struct Records {
    directory: PathBuf,
    runtime: Runtime,
}

impl Records {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-queued-retirement-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let mut record = crate::helper_start_fixture::record();
        // SAFETY: geteuid has no preconditions.
        record.identity.uid = unsafe { libc::geteuid() };
        let runtime = Runtime { path: directory.clone(), generation: record.identity.generation.clone() };
        runtime.save_session(&record).unwrap();
        Self { directory, runtime }
    }

    fn retire(&self) {
        let mut record = self.runtime.session().unwrap();
        record.disabled = true;
        self.runtime.save_session(&record).unwrap();
    }
}

impl Drop for Records {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.runtime.path.with_extension("disabled"));
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test]
async fn retirement_prevents_a_queued_mutation_even_when_configuration_stays_enabled() {
    let records = Records::new();
    let mut fixture = Fixture::new().await;
    let mut workers = fixture.fill_slots().await;
    let mut manager = fixture.manager.clone();
    manager.actions = manager.actions.with_generation(records.runtime.clone());
    let values = ["EXAMPLE=1".into()];
    let mut queued = Box::pin(manager.set_environment(&values));
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(20)) => {},
        result = &mut queued => panic!("queued mutation completed: {result:?}"),
    }
    records.retire();
    fixture.state.replies.add_permits(4);
    assert!(tokio::time::timeout(Duration::from_secs(1), queued).await.unwrap().is_err());
    while let Some(result) = workers.join_next().await { result.unwrap().unwrap(); }
    assert!(fixture.entered.try_recv().is_err());
    assert!(manager.actions.check().is_err());
}

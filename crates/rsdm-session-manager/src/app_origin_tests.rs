//! Original process capture requires the current launch invocation and main PID.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::atomic::Ordering};

use super::*;

#[path = "window_dispatch_fixture.rs"]
mod fixture;

struct Records { directory: PathBuf, runtime: Runtime }

impl Records {
    fn new(session: &crate::runtime::SessionRecord) -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-origin-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::create_at(&directory, crate::helper_start_fixture::GENERATION).unwrap();
        let mut session = session.clone();
        // SAFETY: geteuid has no preconditions.
        session.identity.uid = unsafe { libc::geteuid() };
        runtime.save_session(&session).unwrap();
        runtime.save_app(&crate::helper_start_fixture::app()).unwrap();
        Self { directory, runtime }
    }
}

impl Drop for Records {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

#[tokio::test]
async fn capture_persists_one_kernel_identity_across_repeat_reads() {
    let fixture = fixture::Fixture::new().await;
    let records = Records::new(&fixture.record);
    let expected = ProcessHandle::open(std::process::id()).unwrap().unwrap().id().unwrap();
    capture_process(&fixture.manager, &records.runtime, &fixture.app.unit).await.unwrap();
    let app = records.runtime.app(&fixture.app.unit).unwrap();
    assert_eq!(app.original_process.as_ref().map(|origin| origin.pidfd_id), expected);
    if let Some(origin) = app.original_process {
        assert_eq!(origin.pid, std::process::id());
        capture_process(&fixture.manager, &records.runtime, &fixture.app.unit).await.unwrap();
        assert_eq!(records.runtime.app(&fixture.app.unit).unwrap().original_process, Some(origin));
    }
}

#[tokio::test]
async fn foreign_processes_invocations_and_retired_generations_cannot_capture_an_origin() {
    let supported = ProcessHandle::open(std::process::id()).unwrap().unwrap().id().unwrap().is_some();
    for invalid in ["main", "control", "processes", "invocation", "zero", "disabled"] {
        let fixture = fixture::Fixture::new().await;
        let records = Records::new(&fixture.record);
        match invalid {
            "main" => fixture.state.main_pid.store(0, Ordering::SeqCst),
            "control" => fixture.state.control_pid.store(std::process::id(), Ordering::SeqCst),
            "processes" => fixture.state.present.store(false, Ordering::SeqCst),
            "invocation" => fixture.state.app_invocation.store(2, Ordering::SeqCst),
            "zero" => fixture.state.app_invocation.store(0, Ordering::SeqCst),
            "disabled" => {
                let mut record = records.runtime.session().unwrap();
                record.disabled = true;
                records.runtime.save_session(&record).unwrap();
            }
            _ => unreachable!(),
        }
        let result = capture_process(&fixture.manager, &records.runtime, &fixture.app.unit).await;
        assert_eq!(result.is_err(), supported, "{invalid}: {result:?}");
        assert!(records.runtime.app(&fixture.app.unit).unwrap().original_process.is_none());
    }
}

#[test]
fn legacy_records_have_no_authority_over_an_escaped_process() {
    let app = crate::helper_start_fixture::app();
    let serialized = toml::to_string(&app).unwrap();
    assert!(!serialized.contains("original_process"));
    assert!(toml::from_str::<crate::runtime::AppRecord>(&serialized).unwrap().original_process.is_none());
}

//! Unclaimed authority files must not survive an interrupted startup.

use std::{os::unix::fs::{PermissionsExt, symlink}, sync::atomic::{AtomicUsize, Ordering}, time::Duration};

use super::*;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rsdm-ice-authority-test-{}-{}",
            std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

struct Completed(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for Completed {
    fn drop(&mut self) { let _ = self.0.take().unwrap().send(()); }
}

#[tokio::test]
async fn cancellation_removes_credentials_created_by_an_unclaimed_blocking_result() {
    let directory = Directory::new();
    let path = directory.0.clone();
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let (completed, completed_rx) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(async move {
        crate::runtime::blocking(move || {
            entered.send(()).unwrap();
            released.recv().unwrap();
            let prepared = prepare(path, vec!["local/test".into()])?;
            // Tuple fields drop in order: signal only after authority cleanup.
            Ok((prepared, Completed(Some(completed))))
        }).await
    });
    tokio::time::timeout(Duration::from_secs(2), entered_rx).await.unwrap().unwrap();
    caller.abort();
    let Err(error) = caller.await else { panic!("authority caller was not cancelled"); };
    assert!(error.is_cancelled());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), completed_rx).await.unwrap().unwrap();
    assert!(!directory.0.join("ICEauthority").exists());
}

#[tokio::test]
async fn malformed_networks_remove_partial_files_and_collisions_preserve_existing_credentials() {
    let directory = Directory::new();
    let path = directory.0.clone();
    assert!(crate::runtime::blocking(move || prepare(path, vec!["local/test".into(), "invalid\0network".into()])).await.is_err());
    let authority = directory.0.join("ICEauthority");
    assert!(!authority.exists());

    let target = directory.0.join("existing");
    fs::write(&target, "existing credentials").unwrap();
    symlink(&target, &authority).unwrap();
    let path = directory.0.clone();
    assert!(crate::runtime::blocking(move || prepare(path, vec!["local/test".into()])).await.is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "existing credentials");
    assert!(fs::symlink_metadata(authority).unwrap().file_type().is_symlink());
}

#[tokio::test]
async fn dropping_a_completed_job_removes_its_unclaimed_credentials() {
    let directory = Directory::new();
    let path = directory.0.clone();
    let job = tokio::task::spawn_blocking(move || prepare(path, vec!["local/test".into()]));
    tokio::time::timeout(Duration::from_secs(2), async {
        while !job.is_finished() { tokio::task::yield_now().await; }
    }).await.unwrap();
    let path = directory.0.join("ICEauthority");
    assert!(path.exists());
    drop(job);
    tokio::time::timeout(Duration::from_secs(2), async {
        while path.exists() { tokio::time::sleep(Duration::from_millis(1)).await; }
    }).await.unwrap();
}

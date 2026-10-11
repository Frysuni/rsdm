//! An open waiter must not authorize work on an unlinked lease inode.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use super::*;

struct Fixture { directory: PathBuf, path: PathBuf }

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-lease-inode-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("app.lock");
        Self { directory, path }
    }

    fn replace(&self) -> File {
        let temporary = self.path.with_extension("replacement");
        let held = acquire(&temporary, &Deadline::default()).unwrap();
        fs::rename(temporary, &self.path).unwrap();
        held
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

async fn opened(metadata: &fs::Metadata) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let descriptors = fs::read_dir("/proc/self/fd").unwrap().filter_map(Result::ok)
                .filter_map(|entry| fs::metadata(entry.path()).ok())
                .filter(|current| current.dev() == metadata.dev() && current.ino() == metadata.ino()).count();
            if descriptors >= 2 { break; }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }).await.unwrap();
}

#[tokio::test]
async fn a_waiter_retries_the_current_inode_and_cannot_overtake_its_new_holder() {
    let fixture = Fixture::new();
    let old = acquire(&fixture.path, &Deadline::default()).unwrap();
    let metadata = old.metadata().unwrap();
    let path = fixture.path.clone();
    let mut waiter = tokio::task::spawn_blocking(move || acquire(&path, &Deadline::default()));
    opened(&metadata).await;
    let replacement = fixture.replace();
    drop(old);
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(20)) => {},
        result = &mut waiter => panic!("waiter acquired an obsolete inode: {result:?}"),
    }
    drop(replacement);
    let held = waiter.await.unwrap().unwrap();
    let current = fs::symlink_metadata(&fixture.path).unwrap();
    assert_eq!((held.metadata().unwrap().dev(), held.metadata().unwrap().ino()), (current.dev(), current.ino()));
}

#[tokio::test]
async fn an_unsafe_replacement_cannot_be_bypassed_with_an_old_open_descriptor() {
    let fixture = Fixture::new();
    let old = acquire(&fixture.path, &Deadline::default()).unwrap();
    let metadata = old.metadata().unwrap();
    let path = fixture.path.clone();
    let waiter = tokio::task::spawn_blocking(move || acquire(&path, &Deadline::default()));
    opened(&metadata).await;
    let _replacement = fixture.replace();
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o644)).unwrap();
    drop(old);
    assert!(matches!(waiter.await.unwrap(), Err(SessionError::State(_))));
}

#[tokio::test]
async fn an_inode_retry_keeps_the_same_shortened_shutdown_budget() {
    let fixture = Fixture::new();
    let old = acquire(&fixture.path, &Deadline::default()).unwrap();
    let metadata = old.metadata().unwrap();
    let path = fixture.path.clone();
    let deadline = Deadline::default();
    deadline.set(crate::processes::monotonic_usec().unwrap() + 30_000_000);
    let shared = deadline.clone();
    let waiter = tokio::task::spawn_blocking(move || acquire(&path, &shared));
    opened(&metadata).await;
    let _replacement = fixture.replace();
    deadline.set(crate::processes::monotonic_usec().unwrap() + 50_000);
    drop(old);
    assert!(matches!(waiter.await.unwrap(), Err(SessionError::Bus(error)) if crate::bus::retryable_error(&error)));
}

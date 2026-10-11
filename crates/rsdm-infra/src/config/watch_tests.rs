use std::{fs, sync::atomic::{AtomicU64, Ordering}};

use super::*;
use rsdm_core::domain::SessionManagerConfig;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

struct ConfigFile {
    directory: PathBuf,
    path: PathBuf,
}

impl ConfigFile {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "rsdm-config-watch-{}-{}", std::process::id(), TEST_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("rsdm.toml");
        fs::write(&path, "[session_manager]\nenabled = true\n").unwrap();
        Self { directory, path }
    }

    async fn replace(&self, text: &str) {
        let temporary = self.directory.join("replacement.toml");
        let path = self.path.clone();
        let text = text.to_string();
        tokio::task::spawn_blocking(move || {
            fs::write(&temporary, text).unwrap();
            fs::rename(temporary, path).unwrap();
        }).await.unwrap();
    }
}

impl Drop for ConfigFile {
    fn drop(&mut self) { fs::remove_dir_all(&self.directory).unwrap(); }
}

async fn changed(receiver: &mut watch::Receiver<SessionManagerUpdate>) -> SessionManagerConfig {
    tokio::time::timeout(Duration::from_secs(3), receiver.changed()).await.unwrap().unwrap();
    receiver.borrow_and_update().config.clone()
}

#[tokio::test]
async fn observes_in_place_edits_and_atomic_replacement() {
    let file = tokio::task::spawn_blocking(ConfigFile::new).await.unwrap();
    let mut receiver = watch_session_config(&file.path).await.unwrap();
    assert!(receiver.borrow_and_update().config.enabled);

    let path = file.path.clone();
    tokio::task::spawn_blocking(move || fs::write(path, "[session_manager]\nenabled = false\n"))
        .await.unwrap().unwrap();
    assert!(!changed(&mut receiver).await.enabled);

    file.replace("[session_manager]\nenabled = true\nextra_env = ['CUSTOM_DISPLAY']\n").await;
    let config = changed(&mut receiver).await;
    assert!(config.enabled);
    assert_eq!(config.extra_env, ["CUSTOM_DISPLAY"]);

    // An unrelated configuration edit must not wake the coordinator.
    file.replace("[session_manager]\nenabled = true\nextra_env = ['CUSTOM_DISPLAY']\n[lock]\nsize = 2\n").await;
    assert!(tokio::time::timeout(POLL_INTERVAL * 3, receiver.changed()).await.is_err());
}

#[tokio::test]
async fn keeps_the_last_valid_policy_and_recovers_after_missing_file() {
    let file = tokio::task::spawn_blocking(ConfigFile::new).await.unwrap();
    let mut receiver = watch_session_config(&file.path).await.unwrap();
    receiver.borrow_and_update();

    file.replace("[session_manager]\nenabled = false\nready_timeout_secs = 9223372036854775807\n").await;
    assert!(tokio::time::timeout(POLL_INTERVAL * 3, receiver.changed()).await.is_err());
    assert!(receiver.borrow().config.enabled);

    let path = file.path.clone();
    tokio::task::spawn_blocking(move || fs::remove_file(path)).await.unwrap().unwrap();
    assert!(tokio::time::timeout(POLL_INTERVAL * 3, receiver.changed()).await.is_err());
    assert!(receiver.borrow().config.enabled);

    file.replace("[session_manager]\nenabled = false\n").await;
    assert!(!changed(&mut receiver).await.enabled);
}

#[tokio::test]
async fn invalid_initial_policy_is_rejected() {
    let file = tokio::task::spawn_blocking(ConfigFile::new).await.unwrap();
    file.replace("[session_manager]\nenabled = 'false'\n").await;
    assert!(matches!(watch_session_config(&file.path).await, Err(ConfigLoadError::Parse { .. })));
}

#[tokio::test]
async fn monitoring_ends_only_after_all_receivers_are_dropped() {
    let file = tokio::task::spawn_blocking(ConfigFile::new).await.unwrap();
    let (sender, receiver) = watch::channel(SessionManagerUpdate::from(SessionManagerConfig::default()));
    let clone = receiver.clone();
    let mut worker = tokio::spawn(monitor(file.path.clone(), sender));
    drop(receiver);
    assert!(tokio::time::timeout(POLL_INTERVAL * 2, &mut worker).await.is_err());
    drop(clone);
    tokio::time::timeout(Duration::from_secs(1), worker).await.unwrap().unwrap();
}

#[tokio::test]
async fn re_enabling_keeps_the_disable_transition_when_the_receiver_skips_values() {
    let file = tokio::task::spawn_blocking(ConfigFile::new).await.unwrap();
    let receiver = watch_session_config(&file.path).await.unwrap();
    file.replace("[session_manager]\nenabled = false\n").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while receiver.borrow().disable_count == 0 { tokio::time::sleep(Duration::from_millis(10)).await; }
    }).await.unwrap();
    file.replace("[session_manager]\nenabled = true\n").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !receiver.borrow().config.enabled { tokio::time::sleep(Duration::from_millis(10)).await; }
    }).await.unwrap();
    let update = receiver.borrow();
    assert!(update.config.enabled);
    assert_eq!(update.disable_count, 1);
}

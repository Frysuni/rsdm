use std::process::Command;

use super::*;
use crate::config::{load_config_async, watch_session_config};

#[test]
fn personal_preferences_are_loaded_only_for_lock() {
    const ROOT: &str = "RSDM_TEST_LOCK_CONFIG_ROOT";
    if let Some(root) = env::var_os(ROOT) {
        check_loaders(&PathBuf::from(root));
        return;
    }
    let root = env::temp_dir().join(format!("rsdm-lock-config-test-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let result = Command::new(env::current_exe().unwrap())
        .args(["--exact", "config::lock::tests::personal_preferences_are_loaded_only_for_lock", "--nocapture"])
        .env(ROOT, &root).env("XDG_CONFIG_HOME", &root).env("HOME", &root)
        .output().unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(result.status.success(), "{}\n{}",
        String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
}

fn check_loaders(root: &Path) {
    let path = root.join("system.toml");
    let directory = root.join("rsdm");
    fs::create_dir(&directory).unwrap();
    let preferences = directory.join("lock.toml");
    fs::write(&path, "[lock]\nsize = 4\npam_service = 'policy'\n").unwrap();
    fs::write(&preferences, "[lock]\nsize = 2\n[lock.design]\ntheme = 'nord'\n").unwrap();

    let base = load_config(&path).unwrap();
    let lock = load_lock_config(&path).unwrap();
    assert_eq!(base.lock.size, Some(4));
    assert_eq!(lock.lock.size, Some(2));
    assert_eq!(lock.lock.design.theme, rsdm_core::domain::ThemePreset::Nord);
    assert_eq!(lock.lock.pam_service, "policy");
    assert_eq!(lock.dm, base.dm);
    assert_eq!(lock.session_manager, base.session_manager);

    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        for text in ["invalid = [", "[lock]\nsize = 0\n"] {
            fs::write(&preferences, text).unwrap();
            assert_eq!(load_config(&path).unwrap(), base);
            assert_eq!(load_config_async(&path).await.unwrap(), base);
            assert!(load_lock_config(&path).unwrap_err().to_string().contains(preferences.to_str().unwrap()));
        }
        let mut policy = watch_session_config(&path).await.unwrap();
        assert!(policy.borrow_and_update().config.enabled);
        fs::write(&path, "[session_manager]\nenabled = false\n").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), policy.changed()).await.unwrap().unwrap();
        assert!(!policy.borrow().config.enabled);
    });
}

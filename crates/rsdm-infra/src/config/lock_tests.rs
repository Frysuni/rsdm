use std::process::Command;

use super::*;
use crate::config::{load_config_async, watch_session_config};

#[test]
fn personal_preferences_are_loaded_only_for_lock() {
    in_config_directory("personal_preferences_are_loaded_only_for_lock", check_loaders);
}

fn in_config_directory(test: &str, check: fn(&Path)) {
    const ROOT: &str = "RSDM_TEST_LOCK_CONFIG_ROOT";
    if let Some(root) = env::var_os(ROOT) {
        check(&PathBuf::from(root));
        return;
    }
    let root = env::temp_dir().join(format!("rsdm-lock-config-test-{}-{test}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let result = Command::new(env::current_exe().unwrap())
        .args(["--exact", &format!("config::lock::tests::{test}"), "--nocapture"])
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


#[test]
fn runtime_save_recovers_corruption_without_rewriting_system_policy() {
    in_config_directory("runtime_save_recovers_corruption_without_rewriting_system_policy", |root| {
        use crate::config::save_lock_runtime_settings;
        use rsdm_core::domain::ThemePreset;
        use std::os::unix::fs::PermissionsExt;

        let path = root.join("system.toml");
        let text = "[lock]\nsize = 4\npam_service = 'admin-policy'\n[lock.design]\nshow_clock = false\n";
        fs::write(&path, text).unwrap();
        let base = load_config(&path).unwrap();
        let preferences = root.join("rsdm/lock.toml");
        fs::create_dir(preferences.parent().unwrap()).unwrap();

        // The same overlay mechanism used by the Lock UI must reject policy injection.
        for overlay in ["[lock]\npam_service = 'injected'\n", "[dm]\nenable = false\n", "broken = ["] {
            fs::write(&preferences, overlay).unwrap();
            assert!(load_lock_config(&path).is_err());
            assert_eq!(load_config(&path).unwrap(), base);
        }
        let mut design = base.lock.design.clone();
        design.theme = ThemePreset::Nord;
        design.show_clock = true;
        save_lock_runtime_settings(&path, &design, None, SecondaryOutput::Black).unwrap();

        let mut expected = base.clone();
        expected.lock.design.theme = ThemePreset::Nord;
        expected.lock.size = None;
        expected.lock.secondary_output = SecondaryOutput::Black;
        assert_eq!(load_lock_config(&path).unwrap(), expected);
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert_eq!(fs::metadata(&preferences).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(preferences.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);

        // A failed rename must preserve the obstruction and remove the temporary file.
        fs::remove_file(&preferences).unwrap();
        fs::create_dir(&preferences).unwrap();
        fs::write(preferences.join("sentinel"), "keep").unwrap();
        assert!(save_lock_runtime_settings(&path, &design, Some(2), SecondaryOutput::Off).is_err());
        assert_eq!(fs::read_to_string(preferences.join("sentinel")).unwrap(), "keep");
        assert_eq!(fs::read_dir(preferences.parent().unwrap()).unwrap().count(), 1);
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        fs::remove_dir_all(&preferences).unwrap();

        save_lock_runtime_settings(&path, &design, Some(2), SecondaryOutput::Off).unwrap();
        expected.lock.size = Some(2);
        expected.lock.secondary_output = SecondaryOutput::Off;
        assert_eq!(load_lock_config(&path).unwrap(), expected);
    });
}

#[test]
fn simultaneous_lock_preference_saves_never_publish_a_partial_configuration() {
    in_config_directory("simultaneous_lock_preference_saves_never_publish_a_partial_configuration", |root| {
        use crate::config::save_lock_runtime_settings;
        use rsdm_core::domain::ThemePreset;
        use std::sync::Barrier;
        use std::thread;

        let path = root.join("system.toml");
        fs::write(&path, "[lock]\nsize = 4\npam_service = 'admin-policy'\n").unwrap();
        let base = load_config(&path).unwrap();
        let mut first = base.clone();
        first.lock.size = None;
        first.lock.design.theme = ThemePreset::Nord;
        first.lock.design.background_speed = 1;
        first.lock.secondary_output = SecondaryOutput::Black;
        let mut second = base.clone();
        second.lock.size = Some(2);
        second.lock.design.theme = ThemePreset::Dracula;
        second.lock.design.background_speed = 9;
        second.lock.secondary_output = SecondaryOutput::Off;

        let save = |config: &AppConfig| {
            save_lock_runtime_settings(&path, &config.lock.design, config.lock.size, config.lock.secondary_output).unwrap();
        };
        save(&first);
        let start = Barrier::new(3);
        thread::scope(|scope| {
            for snapshot in [&first, &second] {
                let save = &save;
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..24 {
                        save(snapshot);
                        thread::yield_now();
                    }
                });
            }
            start.wait();
            for _ in 0..256 {
                let observed = load_lock_config(&path).unwrap();
                assert!(observed == first || observed == second, "mixed preference snapshot: {observed:?}");
                thread::yield_now();
            }
        });
        let final_config = load_lock_config(&path).unwrap();
        assert!(final_config == first || final_config == second);
        assert_eq!(load_config(&path).unwrap(), base);
        assert_eq!(fs::read_dir(root.join("rsdm")).unwrap().count(), 1);
    });
}

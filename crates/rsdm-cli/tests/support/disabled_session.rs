//! Disabled commands bypass an existing coordinator and keep ordinary process behavior.

use super::*;

fn disable(tools: &SessionTools) {
    fs::write(tools.root.join("config.toml"), "[session_manager]\nenabled = false\n").unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_hooks_and_requests_leave_the_existing_coordinator_untouched() {
    let tools = SessionTools::new(false).await;
    disable(&tools);
    for args in [
        vec!["session", "finalize", "INVALID NAME"],
        vec!["session", "stop"],
        vec!["session", "cancel"],
        vec!["session", "app-stop", "--generation", "../invalid", "--unit", "../invalid"],
        vec!["session", "cleanup", "--generation", "../invalid"],
        vec!["session", "status"],
    ] {
        let mut command = tools.command(&args);
        command.env("RSDM_SESSION_GENERATION", "invalid generation");
        let output = command.output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    }
    assert!(tools.calls.lock().unwrap().is_empty());
    assert!(!tools.root.join("rsdm").exists());
    let output = tools.run(&["session", "status"]);
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "disabled: session manager is disabled\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_apps_ignore_shutdown_policy_and_preserve_arguments_environment_and_directory() {
    let tools = SessionTools::new(false).await;
    disable(&tools);
    let mut command = tools.command(&["app", "--shutdown-timeout", "0", "--shutdown-method", "xsmp",
        "--quit-command", "'", "--", "/bin/sh", "-c", "pwd; printf '<%s>\\n' \"$RSDM_SESSION_GENERATION\" \"$@\"; exit 7",
        "app", "$HOME", "", "two words", "--flag"]);
    command.current_dir(&tools.root).env("RSDM_SESSION_GENERATION", "invalid generation");
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), format!("{}\n<invalid generation>\n<$HOME>\n<>\n<two words>\n<--flag>\n", tools.root.display()));
    assert!(tools.calls.lock().unwrap().is_empty());
    assert!(!tools.root.join("rsdm").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disabled_app_executor_preserves_the_process_id_and_never_contacts_the_manager() {
    let tools = SessionTools::new(false).await;
    disable(&tools);
    let mut command = tools.command(&["session", "app-exec", "--generation", "../invalid", "--unit", "../invalid",
        "--", "/bin/sh", "-c", "kill -PIPE \"$$\"; printf '%s\\n' \"$$\"; exit 7"]);
    command.stdout(Stdio::piped());
    let child = command.spawn().unwrap();
    let pid = child.id();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), pid.to_string());
    assert!(tools.calls.lock().unwrap().is_empty());
    assert!(!tools.root.join("rsdm").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_start_ignores_provider_options_and_returns_the_original_exit_status() {
    let tools = SessionTools::new(false).await;
    disable(&tools);
    let output = tools.run(&["session", "start", "--mode", "external", "--native-unit", "missing.service",
        "--logout-command", "'", "--", "/bin/sh", "-c", "exit 7"]);
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    assert!(tools.calls.lock().unwrap().is_empty());
    assert!(!tools.root.join("rsdm").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_session_management_keeps_power_on_logind_despite_a_generation_hint() {
    for (action, accepted) in [("reboot", true), ("poweroff", false)] {
        let tools = SessionTools::new(false).await;
        disable(&tools);
        let output = tools.run(&["power", action]);
        assert_eq!(output.status.success(), accepted, "{output:?}");
        assert_eq!(*tools.calls.lock().unwrap(), [format!("logind {action}")]);
    }
}

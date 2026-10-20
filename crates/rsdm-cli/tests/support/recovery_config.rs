//! Automatic recovery reads configuration only from a validated user record.

use super::*;

fn write_record(tools: &SessionTools, generation: &str) {
    let directory = tools.root.join("rsdm/sessions").join(GENERATION);
    fs::create_dir_all(directory.join("apps")).unwrap();
    for path in [tools.root.join("rsdm"), tools.root.join("rsdm/sessions"), directory.clone(), directory.join("apps")] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = tools.root.join("recorded.toml");
    fs::write(&config, "[session_manager]\nenabled = false\n").unwrap();
    let record = format!(
        "anchor_unit = \"rsdm-session-{GENERATION}.service\"\n\
         compositor_invocation = []\nprovider = \"managed\"\nowns_targets = false\nphase = \"closed\"\n\
         config_path = {config:?}\n[identity]\ngeneration = {generation:?}\n\
         login_session_id = \"test\"\nuid = {}\n",
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() },
    );
    let path = directory.join("session.toml");
    fs::write(&path, record).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recorded_recovery_observes_the_saved_configuration_before_contacting_the_manager() {
    let tools = SessionTools::new(false).await;
    write_record(&tools, GENERATION);
    let output = tools.run(&["session", "cleanup", "--generation", GENERATION, "--recorded-config"]);
    assert!(output.status.success(), "{output:?}");
    assert!(tools.calls.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recorded_recovery_rejects_a_generation_mismatch_before_reading_its_configuration() {
    let tools = SessionTools::new(false).await;
    write_record(&tools, "ffffffffffffffffffffffffffffffff");
    let output = tools.run(&["session", "cleanup", "--generation", GENERATION, "--recorded-config"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("session record generation mismatch"), "{output:?}");
    assert!(tools.calls.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recorded_recovery_with_no_record_leaves_no_runtime_state_or_bus_requests() {
    let tools = SessionTools::new(false).await;
    let output = tools.run(&["session", "cleanup", "--generation", GENERATION, "--recorded-config"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!tools.root.join("rsdm").exists());
    assert!(tools.calls.lock().unwrap().is_empty());
}

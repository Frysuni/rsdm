use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_commands_use_the_handshake_generation_instead_of_an_environment_hint() {
    let tools = SessionTools::new(false).await;
    for args in [
        vec!["session", "finalize"], vec!["session", "cancel"],
        vec!["app", "--", "/bin/true"], vec!["session", "stop"],
    ] {
        let output = tools.command(&args).env("RSDM_SESSION_GENERATION", "stale-hint").output().unwrap();
        if args.last() == Some(&"stop") {
            assert_eq!(output.status.code(), Some(1));
        } else {
            assert!(output.status.success(), "{output:?}");
        }
    }
    let calls = tools.calls.lock().unwrap();
    for expected in [format!("finalize {GENERATION}"), format!("cancel {GENERATION}"), format!("stop {GENERATION} logout")] {
        assert!(calls.contains(&expected), "{calls:?}");
    }
    assert!(calls.iter().any(|call| call.starts_with("launch ")), "{calls:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn application_launch_rejects_a_working_directory_that_dbus_cannot_represent() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let tools = SessionTools::new(false).await;
    let directory = tools.root.join(OsString::from_vec(b"working-\xff".to_vec()));
    fs::create_dir(&directory).unwrap();
    fs::create_dir(directory.to_string_lossy().as_ref()).unwrap();
    let output = tools.command(&["app", "--", "/bin/true"]).current_dir(&directory).output().unwrap();

    assert!(!output.status.success(), "lossy working directory was accepted: {output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("not valid UTF-8"), "{output:?}");
    assert!(tools.calls.lock().unwrap().is_empty());

    let output = tools.command(&["app", "--", "/bin/true"])
        .current_dir(directory.to_string_lossy().as_ref()).output().unwrap();
    assert!(output.status.success(), "valid UTF-8 directory was rejected: {output:?}");
    assert!(tools.calls.lock().unwrap().iter().any(|call| call.starts_with("launch ")));
}

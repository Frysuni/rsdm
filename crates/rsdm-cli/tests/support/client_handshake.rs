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

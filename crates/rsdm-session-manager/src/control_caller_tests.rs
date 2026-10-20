use super::*;

#[tokio::test]
async fn shared_user_units_follow_only_loginds_current_graphical_login() {
    let identity = SessionIdentity {
        generation: "0123456789abcdef0123456789abcdef".into(),
        login_session_id: "graphical-login".into(), desktop_entry_id: None,
        uid: unsafe { libc::geteuid() },
    };
    let context = crate::control_test_authority::Context::create(identity.clone()).await;
    for (display, expected) in [("", false), ("other-login", false), ("graphical-login", true), ("new-login", false)] {
        context.set_display(display).await;
        assert_eq!(Caller::belongs_to_display(&context.logind, &identity).await.unwrap(), expected);
    }
}

#[test]
fn missing_credentials_and_another_uid_cannot_establish_a_caller() {
    let uid = unsafe { libc::geteuid() };
    let credentials = ConnectionCredentials::default().set_process_id(std::process::id());
    assert!(Caller::from_credentials(&credentials, uid).is_err());
    let credentials = credentials.set_unix_user_id(uid.wrapping_add(1));
    assert!(Caller::from_credentials(&credentials, uid).is_err());
    for pid in [None, Some(0)] {
        let mut credentials = ConnectionCredentials::default().set_unix_user_id(uid);
        if let Some(pid) = pid { credentials = credentials.set_process_id(pid); }
        assert!(Caller::from_credentials(&credentials, uid).is_err());
    }
}

#[tokio::test]
async fn a_pinned_caller_cannot_survive_its_process_exiting() {
    let uid = unsafe { libc::geteuid() };
    let mut child = tokio::process::Command::new("sleep").arg("30").kill_on_drop(true).spawn().unwrap();
    let credentials = ConnectionCredentials::default().set_unix_user_id(uid).set_process_id(child.id().unwrap());
    let caller = Caller::from_credentials(&credentials, uid).unwrap();
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    assert!(caller.check_alive().is_err());
    assert!(Caller::from_credentials(&credentials, uid).is_err());
}

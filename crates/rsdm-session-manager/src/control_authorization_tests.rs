use rsdm_core::domain::SessionPhase;
use crate::SessionStatusDetails;

use super::*;

async fn endpoint() -> (Endpoint, tokio::sync::mpsc::Receiver<Request>) {
    Endpoint::for_test(super::super::super::identity::SessionIdentity {
        generation: launch().generation, login_session_id: "test".into(), desktop_entry_id: None,
        uid: unsafe { libc::geteuid() },
    }).await
}

async fn assert_all_requests_denied(endpoint: Endpoint, receiver: tokio::sync::mpsc::Receiver<Request>) {
    let (_server, connection, mut received) = connect_endpoint(endpoint, receiver).await;
    let proxy = proxy(&connection).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        let results = [
            proxy.call::<_, _, String>("Handshake", &()).await.map(|_| ()),
            proxy.call::<_, _, SessionStatus>("Attach", &()).await.map(|_| ()),
            proxy.call::<_, _, SessionStatus>("Status", &()).await.map(|_| ()),
            proxy.call::<_, _, SessionStatusDetails>("StatusDetails", &()).await.map(|_| ()),
            proxy.call::<_, _, String>("Launch", &(launch(),)).await.map(|_| ()),
            proxy.call::<_, _, ()>("Finalize", &(launch().generation, Vec::<(String, String)>::new())).await,
            proxy.call::<_, _, StopOutcome>("Stop", &(launch().generation, "logout")).await.map(|_| ()),
            proxy.call::<_, _, ()>("Cancel", &(launch().generation,)).await,
            proxy.call::<_, _, Vec<String>>("XsmpPrepare", &(launch().generation, "", Vec::<String>::new(), true)).await.map(|_| ()),
        ];
        for result in results {
            assert!(result.unwrap_err().to_string().contains("AccessDenied"));
        }
    }).await.unwrap();
    assert!(received.try_recv().is_err());
}

#[tokio::test]
async fn another_login_of_the_same_uid_cannot_read_or_mutate_the_session() {
    let (endpoint, received) = endpoint().await;
    let mut identity = endpoint.verifier.identity.clone();
    endpoint.verifier.context.as_ref().unwrap().set_display(&identity.login_session_id).await;
    identity.login_session_id = "another-login".into();
    endpoint.verifier.context.as_ref().unwrap().set_login(identity).await;
    assert_all_requests_denied(endpoint, received).await;
}

#[tokio::test]
async fn disabled_and_closed_records_reject_each_control_method() {
    for disabled in [true, false] {
        let (endpoint, received) = endpoint().await;
        let runtime = &endpoint.verifier.context.as_ref().unwrap().runtime;
        let mut record = runtime.session().unwrap();
        if disabled { record.disabled = true; } else { record.phase = SessionPhase::Closed; }
        runtime.save_session(&record).unwrap();
        assert_all_requests_denied(endpoint, received).await;
    }
}

#[tokio::test]
async fn closing_admission_interrupts_a_hung_logind_authorization() {
    let (endpoint, received) = endpoint().await;
    let context = endpoint.verifier.context.clone().unwrap();
    context.pending_login.store(true, std::sync::atomic::Ordering::SeqCst);
    let admission = endpoint.admission.clone();
    let (_server, connection, _received) = connect_endpoint(endpoint, received).await;
    let worker = tokio::spawn(async move {
        proxy(&connection).await.unwrap().call::<_, _, SessionStatus>("Status", &()).await
    });
    tokio::time::timeout(Duration::from_secs(2), context.login_entered.notified()).await.unwrap();
    admission.close();
    let result = tokio::time::timeout(Duration::from_millis(500), worker).await.unwrap().unwrap();
    assert!(result.unwrap_err().to_string().contains("disabled"));
}

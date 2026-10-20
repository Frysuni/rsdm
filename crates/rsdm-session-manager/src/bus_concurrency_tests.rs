//! The user-manager request budget covers reads and mutations across clones.

use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use super::*;

#[path = "bus_retirement_tests.rs"]
mod retirement;

struct State {
    entered: tokio::sync::mpsc::UnboundedSender<&'static str>,
    replies: tokio::sync::Semaphore,
    in_flight: AtomicUsize,
    peak: AtomicUsize,
}

struct InFlight<'a>(&'a State);

impl Drop for InFlight<'_> {
    fn drop(&mut self) { self.0.in_flight.fetch_sub(1, Ordering::SeqCst); }
}

impl State {
    async fn reply(&self, method: &'static str) {
        let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        let _guard = InFlight(self);
        self.peak.fetch_max(current, Ordering::SeqCst);
        self.entered.send(method).unwrap();
        self.replies.acquire().await.unwrap().forget();
    }
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    async fn get_unit_processes(&self, _unit: &str) -> Vec<(String, u32, String)> {
        self.0.reply("GetUnitProcesses").await;
        Vec::new()
    }

    async fn set_environment(&self, _values: Vec<String>) {
        self.0.reply("SetEnvironment").await;
    }

    async fn unset_environment(&self, _names: Vec<String>) {
        self.0.reply("UnsetEnvironment").await;
    }

    async fn unref_unit(&self, _unit: &str) {
        self.0.reply("UnrefUnit").await;
    }
}

struct Fixture {
    manager: UserManager,
    entered: tokio::sync::mpsc::UnboundedReceiver<&'static str>,
    state: Arc<State>,
    _server: Connection,
}

impl Fixture {
    async fn new() -> Self {
        let (sender, entered) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(State { entered: sender, replies: tokio::sync::Semaphore::new(0),
            in_flight: AtomicUsize::new(0), peak: AtomicUsize::new(0) });
        let shared = state.clone();
        let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server_socket.into_std().unwrap()).p2p().server(zbus::Guid::generate())
                .serve_at(MANAGER_PATH, Manager(shared)).build().await.unwrap()
        });
        let connection = zbus::connection::Builder::unix_stream(client_socket.into_std().unwrap()).p2p().build().await.unwrap();
        Self { manager: UserManager::with_connection(connection), entered, state, _server: server.await.unwrap() }
    }

    async fn next_call(&mut self) -> &'static str {
        tokio::time::timeout(Duration::from_secs(2), self.entered.recv()).await.unwrap().unwrap()
    }

    async fn fill_slots(&mut self) -> tokio::task::JoinSet<Result<(), SessionError>> {
        let mut workers = tokio::task::JoinSet::new();
        for _ in 0..4 {
            let manager = self.manager.clone();
            workers.spawn(async move { manager.processes("example.service").await.map(|_| ()) });
        }
        for _ in 0..4 { assert_eq!(self.next_call().await, "GetUnitProcesses"); }
        workers
    }
}

#[tokio::test]
async fn reads_and_mutations_share_four_request_slots_across_clones() {
    let mut fixture = Fixture::new().await;
    let mut workers = tokio::task::JoinSet::new();
    for index in 0..12 {
        let manager = fixture.manager.clone();
        workers.spawn(async move {
            match index % 4 {
                0 => manager.processes("example.service").await.map(|_| ()),
                1 => manager.set_environment(&["EXAMPLE=1".into()]).await,
                2 => manager.unset_environment(&["EXAMPLE".into()]).await,
                _ => manager.unref("example.service").await,
            }
        });
    }
    for _ in 0..4 { fixture.next_call().await; }
    assert!(tokio::time::timeout(Duration::from_millis(30), fixture.entered.recv()).await.is_err());
    assert_eq!(fixture.state.peak.load(Ordering::SeqCst), 4);

    fixture.state.replies.add_permits(12);
    while let Some(result) = workers.join_next().await { result.unwrap().unwrap(); }
    assert_eq!(fixture.state.peak.load(Ordering::SeqCst), 4);
    assert_eq!(fixture.state.in_flight.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_queued_mutation_expires_without_being_sent_and_releases_its_waiter() {
    let mut fixture = Fixture::new().await;
    let mut workers = fixture.fill_slots().await;
    let mut queued = fixture.manager.clone();
    queued.deadline = Deadline::default();
    queued.deadline.set(crate::processes::monotonic_usec().unwrap() + 30_000);
    assert!(queued.set_environment(&["EXAMPLE=1".into()]).await.is_err());
    assert!(fixture.entered.try_recv().is_err());

    fixture.state.replies.add_permits(5);
    while let Some(result) = workers.join_next().await { result.unwrap().unwrap(); }
    fixture.manager.set_environment(&["EXAMPLE=2".into()]).await.unwrap();
    assert_eq!(fixture.next_call().await, "SetEnvironment");
    assert!(fixture.entered.try_recv().is_err());
}

#[tokio::test]
async fn dropping_a_queued_mutation_does_not_dispatch_it_later() {
    let mut fixture = Fixture::new().await;
    let mut workers = fixture.fill_slots().await;
    let values = ["EXAMPLE=1".into()];
    let mut queued = Box::pin(fixture.manager.set_environment(&values));
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(10)) => {},
        result = &mut queued => panic!("queued mutation completed: {result:?}"),
    }
    drop(queued);

    fixture.state.replies.add_permits(5);
    while let Some(result) = workers.join_next().await { result.unwrap().unwrap(); }
    fixture.manager.unset_environment(&["EXAMPLE".into()]).await.unwrap();
    assert_eq!(fixture.next_call().await, "UnsetEnvironment");
    assert!(fixture.entered.try_recv().is_err());
}

#[tokio::test]
async fn a_revised_deadline_is_checked_when_a_queued_request_gets_a_slot() {
    let mut fixture = Fixture::new().await;
    let mut workers = fixture.fill_slots().await;
    let mut manager = fixture.manager.clone();
    manager.deadline = Deadline::default();
    let values = ["EXAMPLE=1".into()];
    let mut queued = Box::pin(manager.set_environment(&values));
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(10)) => {},
        result = &mut queued => panic!("queued mutation completed: {result:?}"),
    }
    manager.deadline.set(1);
    workers.abort_all();
    while let Some(result) = workers.join_next().await { assert!(result.unwrap_err().is_cancelled()); }
    assert!(queued.await.is_err());
    fixture.state.replies.add_permits(5);
    fixture.manager.unset_environment(&["EXAMPLE".into()]).await.unwrap();
    assert_eq!(fixture.next_call().await, "UnsetEnvironment");
    assert!(fixture.entered.try_recv().is_err());
}

#[tokio::test]
async fn disabling_cancels_inflight_reads_and_never_dispatches_a_queued_mutation() {
    let mut fixture = Fixture::new().await;
    let mut workers = fixture.fill_slots().await;
    let manager = fixture.manager.clone();
    workers.spawn(async move { manager.set_environment(&["EXAMPLE=1".into()]).await });
    tokio::task::yield_now().await;
    fixture.manager.actions.close();

    tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(result) = workers.join_next().await { assert!(result.unwrap().is_err()); }
    }).await.unwrap();
    fixture.state.replies.add_permits(4);
    tokio::time::timeout(Duration::from_secs(1), async {
        while fixture.state.in_flight.load(Ordering::SeqCst) > 0 { tokio::task::yield_now().await; }
    }).await.unwrap();
    assert!(fixture.entered.try_recv().is_err());
}

#[tokio::test]
async fn cancelling_an_accepted_mutation_does_not_claim_to_undo_it() {
    let mut fixture = Fixture::new().await;
    let manager = fixture.manager.clone();
    let worker = tokio::spawn(async move { manager.set_environment(&["EXAMPLE=1".into()]).await });
    assert_eq!(fixture.next_call().await, "SetEnvironment");
    fixture.manager.actions.close();
    assert!(tokio::time::timeout(Duration::from_secs(1), worker).await.unwrap().unwrap().is_err());
    assert_eq!(fixture.state.in_flight.load(Ordering::SeqCst), 1);
    fixture.state.replies.add_permits(1);
    tokio::time::timeout(Duration::from_secs(1), async {
        while fixture.state.in_flight.load(Ordering::SeqCst) > 0 { tokio::task::yield_now().await; }
    }).await.unwrap();
    assert!(fixture.entered.try_recv().is_err());
}

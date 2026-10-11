use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use zbus::zvariant::OwnedObjectPath;

use super::*;

struct Manager;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, unit: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        if unit != crate::attach::UNIT { return Err(zbus::fdo::Error::Failed("unexpected unit lookup".into())); }
        Ok(OwnedObjectPath::try_from("/coordinator").unwrap())
    }
}

struct Unit;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> { vec![1; 16] }
}

struct Fixture { directory: PathBuf, runtime: Runtime, manager: UserManager, _server: zbus::Connection }

impl Fixture {
    async fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-quit-anchor-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::create_at(&directory, crate::helper_start_fixture::GENERATION).unwrap();
        let (server, client) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server).p2p().server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager).unwrap()
                .serve_at("/coordinator", Unit).unwrap().build().await.unwrap()
        });
        let client = zbus::connection::Builder::unix_stream(client).p2p().build().await.unwrap();
        Self { directory, runtime, manager: UserManager::with_connection(client), _server: server.await.unwrap() }
    }

    fn save(&self, provider: &str, coordinator: Option<(String, Vec<u8>)>) {
        let mut record = crate::helper_start_fixture::record();
        // SAFETY: geteuid has no preconditions.
        record.identity.uid = unsafe { libc::geteuid() };
        record.provider = provider.into();
        record.coordinator_unit = coordinator;
        self.runtime.save_session(&record).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

#[tokio::test]
async fn owned_sessions_use_the_anchor_and_attached_sessions_use_the_verified_coordinator() {
    let fixture = Fixture::new().await;
    fixture.save("managed", None);
    assert_eq!(quit_anchor(&fixture.manager, &fixture.runtime).await.unwrap(), crate::helper_start_fixture::record().anchor_unit);
    fixture.save("partial", Some((crate::attach::UNIT.into(), vec![1; 16])));
    assert_eq!(quit_anchor(&fixture.manager, &fixture.runtime).await.unwrap(), crate::attach::UNIT);
}

#[tokio::test]
async fn attached_sessions_reject_missing_foreign_or_replaced_coordinator_ownership() {
    let fixture = Fixture::new().await;
    for coordinator in [None, Some(("foreign.service".into(), vec![1; 16])),
        Some((crate::attach::UNIT.into(), Vec::new())), Some((crate::attach::UNIT.into(), vec![0; 16])),
        Some((crate::attach::UNIT.into(), vec![2; 16]))]
    {
        fixture.save("partial", coordinator);
        assert!(quit_anchor(&fixture.manager, &fixture.runtime).await.is_err());
    }
}

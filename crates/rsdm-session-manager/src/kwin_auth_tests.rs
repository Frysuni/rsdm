//! Version discovery must come from the broker-pinned Wayland compositor process.

use std::os::fd::{FromRawFd, OwnedFd};

use super::*;
use crate::{kwin, wayfire::tests::Fixture};

struct Information { version: &'static str, reads: Arc<std::sync::atomic::AtomicUsize> }

#[zbus_macros::interface(name = "org.kde.KWin")]
impl Information {
    #[zbus(name = "supportInformation")]
    fn support_information(&self, #[zbus(header)] header: zbus::message::Header<'_>) -> String {
        assert_eq!(header.destination().unwrap().as_str(), ":1.1");
        assert!(header.primary().flags().contains(zbus::message::Flags::NO_AUTO_START));
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("KWin version: {}\n", self.version)
    }
}

#[tokio::test]
async fn version_reads_require_the_current_broker_pinned_compositor() {
    for invalid in ["valid", "numeric", "uid", "pid", "owner", "version", "gate"] {
        let mut broker = Broker::current();
        if invalid != "numeric" {
            // SAFETY: pidfd_open pins this live test process; ownership transfers to OwnedFd.
            let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, std::process::id(), 0) };
            assert!(descriptor >= 0);
            broker.descriptor = Some(unsafe { OwnedFd::from_raw_fd(descriptor as i32) });
        }
        if invalid == "uid" { broker.uid = broker.uid.map(|uid| uid ^ 1); }
        broker.replaced = invalid == "owner";
        let (manager, server, state) = fixture(Reply::Success, broker).await;
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        server.object_server().at("/KWin", Information {
            version: if invalid == "version" { "6.3.6" } else { "6.7.1" }, reads: reads.clone(),
        }).await.unwrap();
        let socket = Fixture::new();
        let mut connection = kwin::Connection::connect(&socket.path, unsafe { libc::geteuid() }).await.unwrap();
        let _peer = socket.accept().await;
        if invalid == "pid" { connection.pid = 0; }
        if invalid == "gate" { manager.actions.close(); }
        let result = connection.authenticate(&manager).await;
        assert_eq!(result.is_ok(), invalid == "valid", "{invalid}: {result:?}");
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), usize::from(matches!(invalid, "valid" | "version")));
        assert!(state.calls.lock().unwrap().is_empty());
    }
}

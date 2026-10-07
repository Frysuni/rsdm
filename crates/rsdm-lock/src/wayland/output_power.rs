//! Serialize bounded output commands without blocking Wayland dispatch.

use std::{
    collections::BTreeSet,
    io,
    process::Command,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const RESTORE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Pending {
    outputs: Vec<String>,
    restore: bool,
    shutdown: bool,
    changed: bool,
}

#[derive(Default)]
struct Requests {
    pending: Mutex<Pending>,
    changed: Condvar,
}

pub(super) struct OutputPower {
    requests: Arc<Requests>,
    worker: Option<JoinHandle<()>>,
}

impl OutputPower {
    pub(super) fn start() -> Option<Self> {
        if std::env::var_os("NIRI_SOCKET").is_none() { return None; }
        match Self::with_command(niri_output_command) {
            Ok(worker) => Some(worker),
            Err(error) => {
                tracing::warn!(%error, "cannot start output power worker; using black backgrounds");
                None
            }
        }
    }

    fn with_command(
        command: impl FnMut(&str, &str, Instant) -> io::Result<()> + Send + 'static,
    ) -> io::Result<Self> {
        let requests = Arc::new(Requests::default());
        let shared = Arc::clone(&requests);
        let worker = thread::Builder::new().name("rsdm-lock-outputs".into())
            .spawn(move || run_worker(&shared, command))?;
        Ok(Self { requests, worker: Some(worker) })
    }

    pub(super) fn power_off(&self, outputs: Vec<String>) {
        let mut pending = self.requests.pending.lock().unwrap_or_else(|error| error.into_inner());
        pending.outputs = outputs;
        pending.changed = true;
        self.requests.changed.notify_one();
    }

    pub(super) fn restore(&self) {
        let mut pending = self.requests.pending.lock().unwrap_or_else(|error| error.into_inner());
        pending.outputs.clear();
        pending.restore = true;
        pending.changed = true;
        self.requests.changed.notify_one();
    }
}

impl Drop for OutputPower {
    fn drop(&mut self) {
        {
            let mut pending = self.requests.pending.lock().unwrap_or_else(|error| error.into_inner());
            pending.outputs.clear();
            pending.restore = true;
            pending.shutdown = true;
            pending.changed = true;
            self.requests.changed.notify_one();
        }
        // Only final cleanup waits: an in-flight command has its own deadline,
        // then all restorations share one budget. No Wayland handler joins.
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!("output power worker stopped unexpectedly");
        }
    }
}

fn next_request(requests: &Requests) -> Pending {
    let mut pending = requests.pending.lock().unwrap_or_else(|error| error.into_inner());
    while !pending.changed {
        pending = requests.changed.wait(pending).unwrap_or_else(|error| error.into_inner());
    }
    std::mem::take(&mut *pending)
}

fn superseded(requests: &Requests) -> bool {
    requests.pending.lock().unwrap_or_else(|error| error.into_inner()).changed
}

fn run_worker(
    requests: &Requests,
    mut command: impl FnMut(&str, &str, Instant) -> io::Result<()>,
) {
    let mut owned = BTreeSet::new();
    let mut fallback_warned = false;
    let mut restore_pending = false;
    loop {
        let pending = next_request(requests);
        restore_pending |= pending.restore;
        if restore_pending {
            if !restore_outputs(&mut owned, &mut command, requests) { continue; }
            restore_pending = false;
        }
        if pending.shutdown { break; }
        for output in pending.outputs {
            if superseded(requests) { break; }
            if owned.contains(&output) { continue; }
            // A timeout or failed reply does not prove that Off had no effect.
            // Keep the restoration obligation even if the output global vanishes.
            owned.insert(output.clone());
            if let Err(error) = command(&output, "off", Instant::now() + COMMAND_TIMEOUT) {
                if error.kind() == io::ErrorKind::NotFound { owned.remove(&output); }
                if !fallback_warned {
                    tracing::warn!(%error, %output, "output power off failed; using black backgrounds");
                    fallback_warned = true;
                }
            }
        }
    }
}

fn restore_outputs(
    owned: &mut BTreeSet<String>,
    command: &mut impl FnMut(&str, &str, Instant) -> io::Result<()>,
    requests: &Requests,
) -> bool {
    let deadline = Instant::now() + RESTORE_TIMEOUT;
    let mut interrupted = false;
    owned.retain(|output| {
        if superseded(requests) {
            interrupted = true;
            return true;
        }
        let until = deadline.min(Instant::now() + COMMAND_TIMEOUT);
        match command(output, "on", until) {
            Ok(()) => false,
            Err(error) => {
                tracing::error!(%error, %output, "failed to restore secondary output");
                true
            }
        }
    });
    !interrupted
}

fn niri_output_command(output: &str, action: &str, deadline: Instant) -> io::Result<()> {
    let status = rsdm_infra::unix::run_command_until(
        Command::new("niri").args(["msg", "output", output, action]), deadline,
    )?;
    if status.success() { return Ok(()); }
    Err(io::Error::other(format!("niri output {action} failed: {status}")))
}

#[cfg(test)]
#[path = "output_power_tests.rs"]
mod tests;

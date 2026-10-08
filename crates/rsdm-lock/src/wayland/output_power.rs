//! Serialize bounded output commands without blocking Wayland dispatch.

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::Command,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const RESTORE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_OUTPUT_NAME_BYTES: usize = 256;
const MAX_OWNERSHIP_BYTES: usize = 4096;

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
        match Self::with_command_and_state(niri_output_command, true) {
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
        Self::with_command_and_state(command, false)
    }

    fn with_command_and_state(
        command: impl FnMut(&str, &str, Instant) -> io::Result<()> + Send + 'static,
        persist: bool,
    ) -> io::Result<Self> {
        let requests = Arc::new(Requests::default());
        let shared = Arc::clone(&requests);
        let initial_owned = persist.then(load_owned_outputs).unwrap_or_default();
        let worker = thread::Builder::new().name("rsdm-lock-outputs".into())
            .spawn(move || run_worker(&shared, initial_owned, command, persist))?;
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
    initial_owned: BTreeSet<String>,
    mut command: impl FnMut(&str, &str, Instant) -> io::Result<()>,
    persist: bool,
) {
    let mut owned = initial_owned;
    let mut fallback_warned = false;
    let mut restore_pending = false;
    if !owned.is_empty() {
        restore_outputs(&mut owned, &mut command, requests);
        if persist { persist_owned_outputs(&owned); }
    }
    loop {
        let pending = next_request(requests);
        restore_pending |= pending.restore;
        if restore_pending {
            if !restore_outputs(&mut owned, &mut command, requests) { continue; }
            if persist { persist_owned_outputs(&owned); }
            restore_pending = false;
        }
        if pending.shutdown { break; }
        for output in pending.outputs {
            if superseded(requests) { break; }
            if owned.contains(&output) { continue; }
            // A timeout or failed reply does not prove that Off had no effect.
            // Keep the restoration obligation even if the output global vanishes.
            owned.insert(output.clone());
            if persist { persist_owned_outputs(&owned); }
            if let Err(error) = command(&output, "off", Instant::now() + COMMAND_TIMEOUT) {
                if error.kind() == io::ErrorKind::NotFound {
                    owned.remove(&output);
                    if persist { persist_owned_outputs(&owned); }
                }
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

fn ownership_path() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    runtime.is_absolute().then(|| runtime.join("rsdm/lock-outputs"))
}

fn load_owned_outputs() -> BTreeSet<String> {
    let Some(path) = ownership_path() else { return BTreeSet::new(); };
    let Ok(file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&path) else { return BTreeSet::new(); };
    let Ok(metadata) = file.metadata() else { return BTreeSet::new(); };
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0 || metadata.len() > MAX_OWNERSHIP_BYTES as u64
    {
        tracing::warn!(path = %path.display(), "ignoring unsafe output ownership record");
        return BTreeSet::new();
    }
    let mut text = String::new();
    if file.take((MAX_OWNERSHIP_BYTES + 1) as u64).read_to_string(&mut text).is_err()
        || text.len() > MAX_OWNERSHIP_BYTES
    {
        return BTreeSet::new();
    }
    text.lines().filter_map(validate_output_name).collect()
}

fn persist_owned_outputs(owned: &BTreeSet<String>) {
    let Some(path) = ownership_path() else { return; };
    if owned.is_empty() {
        let _ = fs::remove_file(path);
        return;
    }
    let Some(parent) = path.parent() else { return; };
    if fs::create_dir_all(parent).is_err() { return; }
    let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    let body = owned.iter().map(|output| format!("{output}\n")).collect::<String>();
    if body.len() > MAX_OWNERSHIP_BYTES { return; }
    let temporary = parent.join(format!(".lock-outputs.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new().create_new(true).write(true)
            .mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&temporary)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() { let _ = fs::remove_file(temporary); }
}

fn validate_output_name(name: &str) -> Option<String> {
    (!name.is_empty() && name.len() <= MAX_OUTPUT_NAME_BYTES
        && !name.chars().any(|ch| ch == '\0' || ch.is_control()))
        .then(|| name.to_string())
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

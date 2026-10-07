//! Cgroup membership and signals that cannot hit a reused process ID.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use super::{SessionError, bus::{UserManager, missing_unit}, runtime::AppRecord};

const SERVICE: &str = "org.freedesktop.systemd1.Service";

pub(super) fn verify_invocation(manager: &UserManager, app: &AppRecord) -> Result<bool, SessionError> {
    match manager.invocation_id(&app.unit) {
        Ok(id) => {
            // An unreferenced unit can lose its invocation identity as its
            // final processes exit. Require both stopped state and an empty
            // cgroup before treating that terminal race as a closed app.
            if id.iter().all(|byte| *byte == 0) && !manager.active(&app.unit)?
                && manager.processes(&app.unit)?.is_empty()
            { return Ok(false); }
            if app.invocation_id.is_empty() || id != app.invocation_id {
                return Err(SessionError::State(format!("{} invocation changed", app.unit)));
            }
            Ok(true)
        }
        Err(SessionError::Bus(error)) if missing_unit(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) fn app_processes(manager: &UserManager, app: &AppRecord) -> Result<Vec<u32>, SessionError> {
    if !verify_invocation(manager, app)? { return Ok(Vec::new()); }
    let control: u32 = match manager.unit_property(&app.unit, SERVICE, "ControlPID") {
        Ok(pid) => pid,
        Err(SessionError::Bus(error)) if missing_unit(&error) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(manager.processes(&app.unit)?.into_iter()
        .map(|(_, pid, _)| pid).filter(|pid| *pid != control).collect())
}

pub(super) fn generation_invocation(
    manager: &UserManager, unit: &str, generation: &str,
) -> Result<Option<Vec<u8>>, SessionError> {
    let result = (|| {
        let id = manager.invocation_id(unit)?;
        let environment: Vec<String> = manager.unit_property(unit, SERVICE, "Environment")?;
        if id.is_empty() || !environment.contains(&format!("{}={generation}", super::identity::GENERATION_ENV))
            || manager.invocation_id(unit)? != id
        {
            return Err(SessionError::State(format!("{unit} does not belong to this generation")));
        }
        Ok(Some(id))
    })();
    match result {
        Err(SessionError::Bus(error)) if missing_unit(&error) => Ok(None),
        other => other,
    }
}

pub(super) fn signal_app(manager: &UserManager, app: &AppRecord, signal: i32) -> Result<(), SessionError> {
    let processes = app_processes(manager, app)?;
    for pid in processes {
        let Some(handle) = ProcessHandle::open(pid)? else { continue; };
        // Opening the pidfd pins an identity; recheck that identity's membership.
        if app_processes(manager, app)?.contains(&pid) {
            handle.signal(signal)?;
        }
    }
    Ok(())
}

pub(super) fn stop_invocation(manager: &UserManager, unit: &str, expected: &[u8]) -> Result<(), SessionError> {
    match manager.invocation_id(unit) {
        Ok(id) if !expected.is_empty() && id == expected => manager.stop(unit, std::time::Duration::from_secs(15)),
        Ok(id) if id.iter().all(|byte| *byte == 0) && !manager.active(unit)?
            && manager.processes(unit)?.is_empty() => Ok(()),
        Ok(_) => Err(SessionError::State(format!("{unit} invocation changed or was never verified"))),
        Err(SessionError::Bus(error)) if missing_unit(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn terminate_main(manager: &UserManager, app: &AppRecord) -> Result<(), SessionError> {
    if !verify_invocation(manager, app)? {
        return Ok(());
    }
    let main: u32 = manager.unit_property(&app.unit, SERVICE, "MainPID")?;
    if main == 0 {
        return signal_app(manager, app, libc::SIGTERM);
    }
    let Some(handle) = ProcessHandle::open(main)? else {
        return signal_app(manager, app, libc::SIGTERM);
    };
    if app_processes(manager, app)?.contains(&main) {
        handle.signal(libc::SIGTERM)?;
    }
    Ok(())
}

pub(super) struct ProcessHandle(OwnedFd);

impl ProcessHandle {
    pub fn open(pid: u32) -> Result<Option<Self>, SessionError> {
        // SAFETY: pidfd_open takes a process ID and flags, and returns a new FD.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ESRCH) { Ok(None) } else { Err(error.into()) };
        }
        // SAFETY: the successful syscall returned a new descriptor we own.
        Ok(Some(Self(unsafe { OwnedFd::from_raw_fd(fd as i32) })))
    }

    #[cfg(feature = "xsmp")]
    pub fn alive(&self) -> Result<bool, SessionError> {
        let mut descriptor = libc::pollfd { fd: self.0.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: descriptor describes the owned pidfd; the poll never blocks.
        let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
        if result < 0 { return Err(std::io::Error::last_os_error().into()); }
        Ok(result == 0)
    }

    pub fn signal(&self, signal: i32) -> Result<(), SessionError> {
        // SAFETY: the owned pidfd identifies the process; no siginfo is supplied.
        let result = unsafe {
            libc::syscall(libc::SYS_pidfd_send_signal, self.0.as_raw_fd(), signal, std::ptr::null::<libc::siginfo_t>(), 0)
        };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) { return Err(error.into()); }
        }
        Ok(())
    }
}

pub(super) fn monotonic_usec() -> Result<u64, SessionError> {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: time is a valid writable timespec.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(time.tv_sec as u64 * 1_000_000 + time.tv_nsec as u64 / 1_000)
}

pub(super) fn require_pidfds() -> Result<(), SessionError> {
    let handle = ProcessHandle::open(std::process::id())?
        .ok_or_else(|| SessionError::State("could not pin the session coordinator process".into()))?;
    handle.signal(0).map_err(|error| SessionError::State(format!("Linux pidfd signals are required: {error}")))
}

#[cfg(test)]
#[path = "processes_tests.rs"]
mod invocation_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pidfd_remains_bound_to_the_original_child_after_exit() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let handle = ProcessHandle::open(child.id()).unwrap().unwrap();
        handle.signal(libc::SIGTERM).unwrap();
        assert!(!child.wait().unwrap().success());
        handle.signal(libc::SIGTERM).unwrap();
    }

    #[test]
    fn shared_deadlines_use_a_monotonic_clock() {
        let first = monotonic_usec().unwrap();
        assert!(monotonic_usec().unwrap() >= first);
    }
}

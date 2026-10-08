//! Credential/environment setup runs only in a newly executed process image.

use std::{convert::Infallible, ffi::CString, fs::File, io, os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd}};

use rsdm_core::ports::SessionLaunchError;

use super::{command::{PreparedCommand, cstring}, session_exec_request::ExecRequest, session_start_gate::wait_reader};

pub fn exec_session(request_fd: RawFd, gate_fd: RawFd) -> Result<Infallible, SessionLaunchError> {
    if request_fd == gate_fd { return Err(SessionLaunchError::Setup("session exec descriptors overlap".into())); }
    let request = ExecRequest::read(File::from(take_descriptor(request_fd)?))?;
    let gate = take_descriptor(gate_fd)?;
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: gate owns the descriptor; metadata is writable storage for fstat.
    check(unsafe { libc::fstat(gate.as_raw_fd(), metadata.as_mut_ptr()) }, "checking session start gate")?;
    // SAFETY: successful fstat initialized the entire stat structure.
    if unsafe { metadata.assume_init() }.st_mode & libc::S_IFMT != libc::S_IFIFO {
        return Err(SessionLaunchError::Setup("session start gate is not a pipe".into()));
    }
    if !wait_reader(gate) { return Err(SessionLaunchError::Setup("session start was aborted".into())); }

    let command = PreparedCommand::new_wrapped_argv(&request.wrapper, &request.command)?;
    configure_session(&request)?;
    let argv = command.argv_ptrs();
    // SAFETY: argv and program remain valid terminated strings through exec.
    unsafe { libc::execvp(command.program.as_ptr(), argv.as_ptr()); }
    Err(SessionLaunchError::Process(format!("executing user session: {}", io::Error::last_os_error())))
}

fn take_descriptor(fd: RawFd) -> Result<OwnedFd, SessionLaunchError> {
    if fd <= libc::STDERR_FILENO { return Err(SessionLaunchError::Setup("invalid session exec descriptor".into())); }
    // SAFETY: set CLOEXEC before adopting only descriptors that exist. They
    // must not survive the final user exec, even if validation later fails.
    check(unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) }, "adopting session exec descriptor")?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn configure_session(request: &ExecRequest) -> Result<(), SessionLaunchError> {
    let username = cstring("username", &request.username)?;
    let home = cstring("home", &request.home)?;
    let environment: Vec<(CString, CString)> = request.environment.iter()
        .map(|(key, value)| Ok((cstring("env key", key)?, cstring("env value", value)?)))
        .collect::<Result<_, SessionLaunchError>>()?;

    // This helper has no inherited PAM module threads or libc locks. Preserve
    // the login environment/group ordering used by the original launcher.
    unsafe {
        check(libc::setsid(), "creating session")?;
        check(libc::clearenv(), "clearing environment")?;
        for (key, value) in &environment {
            check(libc::setenv(key.as_ptr(), value.as_ptr(), 1), "setting environment")?;
        }
        check(libc::initgroups(username.as_ptr(), request.gid), "initializing groups")?;
        check(libc::setgid(request.gid), "setting group")?;
        check(libc::setuid(request.uid), "setting user")?;
        // Match login(1): missing home starts in / without rewriting HOME.
        if libc::chdir(home.as_ptr()) != 0 { check(libc::chdir(c"/".as_ptr()), "changing directory")?; }
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    Ok(())
}

fn check(result: libc::c_int, operation: &str) -> Result<(), SessionLaunchError> {
    if result < 0 { return Err(SessionLaunchError::Setup(format!("{operation}: {}", io::Error::last_os_error()))); }
    Ok(())
}

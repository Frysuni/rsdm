//! systemd-backed Wayland session lifecycle.
//!
//! The compositor, graphical targets, exported environment and transient app
//! units are bound to one lifetime and torn down together.

mod env;
mod readiness;
mod systemd;
mod unit_name;

use std::{
    os::raw::c_int,
    process::{Child, Command},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use rsdm_core::domain::SessionManagerConfig;
use thiserror::Error;

use env::{
    collect_present, command_environment, export, manager_wayland_display,
    unset_environment_command,
};
use readiness::{SessionOwnership, wait_and_anchor};
use systemd::{
    ANCHOR_UNIT, ManagedUnitKind, SESSION_TARGET, anchor_graphical_session,
    anchored_teardown_commands, best_effort_owned, exit_status_code, release_anchor,
    release_anchor_commands, stop_unit, systemd_run_args, unit_is_active,
};
use unit_name::unique_unit_name;

static TERMINATE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("no compositor command was given")]
    EmptyCommand,
    #[error("a graphical session is already active; refusing to replace it")]
    SessionAlreadyActive,
    #[error("failed to activate the graphical session")]
    ActivationFailed,
    #[error("failed to spawn compositor: {0}")]
    Spawn(std::io::Error),
    #[error("failed to wait for compositor: {0}")]
    Wait(std::io::Error),
    #[error(
        "graphical-session.target is not active; `rsdm app` must run inside a graphical session"
    )]
    NoGraphicalSession,
}

/// Start `compositor` as the graphical session and supervise it until it exits.
/// Returns the compositor's exit code.
pub fn start(compositor: &[String], cfg: &SessionManagerConfig) -> Result<i32, SessionError> {
    let program = compositor.first().ok_or(SessionError::EmptyCommand)?;
    if unit_is_active(SESSION_TARGET) || unit_is_active(ANCHOR_UNIT) {
        return Err(SessionError::SessionAlreadyActive);
    }
    TERMINATE.store(false, Ordering::SeqCst);
    install_signal_handlers();
    tracing::info!(
        program,
        argv = ?compositor,
        ready_timeout_secs = cfg.ready_timeout_secs,
        extra_env = ?cfg.extra_env,
        "starting managed graphical session"
    );

    // A crashed session may leave the fixed anchor active.
    release_anchor();

    let session_env = command_environment();
    tracing::debug!(
        env_count = session_env.len(),
        env_names = ?session_env.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        "captured compositor environment"
    );
    export(&collect_present(&[], &[]));
    let previous_display = manager_wayland_display();
    let unit = unique_unit_name(ManagedUnitKind::Session, program);
    let mut child = Command::new("systemd-run")
        .envs(session_env.iter().map(|(key, value)| (key, value)))
        .args(systemd_run_args(
            ManagedUnitKind::Session,
            &unit,
            &session_env,
        ))
        .args(compositor)
        .spawn()
        .map_err(SessionError::Spawn)?;
    tracing::info!(unit, "started Wayland compositor unit");

    let session_running = Arc::new(AtomicBool::new(true));
    let ready_running = Arc::clone(&session_running);
    let ready_cfg = cfg.clone();
    let ready_unit = format!("{unit}.service");
    let ready = thread::spawn(move || {
        wait_and_anchor(&ready_cfg, &ready_unit, &ready_running, previous_display)
    });

    let result = supervise_systemd_run(&mut child, &unit);
    if result.is_err() {
        stop_unit(&unit);
    }
    session_running.store(false, Ordering::SeqCst);
    let ownership = ready.join().unwrap_or(SessionOwnership::Unmanaged);
    match &result {
        Ok(code) => tracing::info!(
            code,
            ?ownership,
            "compositor exited; tearing down graphical session"
        ),
        Err(error) => tracing::warn!(
            %error,
            ?ownership,
            "compositor supervision failed; tearing down graphical session"
        ),
    }
    teardown(cfg, ownership);
    result
}

/// Run `argv` in a transient unit tied to `graphical-session.target`.
pub fn run_app(argv: &[String]) -> Result<i32, SessionError> {
    let program = argv.first().ok_or(SessionError::EmptyCommand)?;
    wait_for_graphical_session()?;
    let unit = unique_unit_name(ManagedUnitKind::App, program);
    let app_env = command_environment();
    tracing::info!(program, unit, argv = ?argv, "launching app as transient systemd user unit");
    tracing::debug!(
        unit,
        env_count = app_env.len(),
        env_names = ?app_env.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        "captured app environment"
    );
    let status = Command::new("systemd-run")
        .envs(app_env.iter().map(|(key, value)| (key, value)))
        .args(systemd_run_args(ManagedUnitKind::App, &unit, &app_env))
        .args(argv)
        .status()
        .map_err(SessionError::Spawn)?;
    let code = exit_status_code(status);
    tracing::info!(unit, code, "app transient unit command finished");
    Ok(code)
}

/// Wait briefly for `graphical-session.target` during session startup; refuse
/// when no graphical session comes up (see [`run_app`]).
fn wait_for_graphical_session() -> Result<(), SessionError> {
    const STARTUP_GRACE: Duration = Duration::from_secs(10);
    if unit_is_active(SESSION_TARGET) {
        return Ok(());
    }
    tracing::info!(
        target = SESSION_TARGET,
        "graphical session is not active yet; waiting for it before launching the app"
    );
    let deadline = Instant::now() + STARTUP_GRACE;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(200));
        if unit_is_active(SESSION_TARGET) {
            return Ok(());
        }
    }
    Err(SessionError::NoGraphicalSession)
}

/// Export the current compositor environment and activate the session.
pub fn finalize(cfg: &SessionManagerConfig, extra_names: &[String]) -> Result<(), SessionError> {
    let pairs = collect_present(&cfg.extra_env, extra_names);
    tracing::debug!(
        exported_names = ?pairs.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        "exporting graphical session environment from the live compositor"
    );
    export(&pairs);
    // The anchor wants both targets independently to avoid an ordering cycle.
    if !unit_is_active(SESSION_TARGET) && !anchor_graphical_session(None) {
        return Err(SessionError::ActivationFailed);
    }
    tracing::info!("graphical session activated");
    Ok(())
}

fn teardown(cfg: &SessionManagerConfig, ownership: SessionOwnership) {
    // Finalize may create our anchor after the readiness timeout expired.
    let ownership = if ownership == SessionOwnership::Unmanaged && unit_is_active(ANCHOR_UNIT) {
        SessionOwnership::Anchored
    } else {
        ownership
    };
    let commands = match ownership {
        SessionOwnership::Anchored => anchored_teardown_commands(cfg),
        SessionOwnership::External => release_anchor_commands(),
        SessionOwnership::Unmanaged => {
            let mut commands = release_anchor_commands();
            commands.push(unset_environment_command(cfg));
            commands
        }
    };
    for args in commands {
        tracing::debug!(?args, "running graphical session teardown command");
        best_effort_owned(&args);
    }
}

/// Supervise systemd-run, forwarding termination requests to the transient unit.
fn supervise_systemd_run(child: &mut Child, unit: &str) -> Result<i32, SessionError> {
    let mut signalled = false;
    loop {
        if TERMINATE.load(Ordering::SeqCst) && !signalled {
            signalled = true;
            tracing::warn!(unit, "termination requested; stopping compositor unit");
            stop_unit(unit);
        }
        match child.try_wait().map_err(SessionError::Wait)? {
            Some(status) => return Ok(exit_status_code(status)),
            None => thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn install_signal_handlers() {
    // SAFETY: the handler only stores into an atomic, which is async-signal-safe.
    unsafe {
        let handler = on_terminate as *const () as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }
}

extern "C" fn on_terminate(_signal: c_int) {
    TERMINATE.store(true, Ordering::SeqCst);
}

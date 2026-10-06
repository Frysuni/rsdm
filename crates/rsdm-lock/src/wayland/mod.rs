use std::{
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use rsdm_core::domain::{AppConfig, SecondaryOutput};
use rsdm_infra::security::MemoryLoginAttemptLimiter;
use rsdm_ui::{Design, LockMenuSettings, LockPending, Menu};
use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    reexports::client::{
        Connection,
        globals::registry_queue_init,
        protocol::{wl_keyboard, wl_output, wl_seat},
    },
    registry::RegistryState,
    seat::SeatState,
    session_lock::{SessionLock, SessionLockState, SessionLockSurface},
    shm::{
        Shm,
        slot::{Buffer, SlotPool},
    },
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::WpFractionalScaleV1,
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

use crate::{model::LockModel, render::Wallpaper, util};

mod authentication;
mod handlers;
mod input;
mod output;
mod protocol;
mod render;

const ANIMATION_FRAME: Duration = Duration::from_millis(66);

struct LockContext {
    font: crate::render::Font,
    design: Design,
    wallpaper: Option<Wallpaper>,
    username: String,
    hibernate_available: bool,
    pam_service: String,
    primary_output: Option<String>,
    secondary_output: SecondaryOutput,
    config_path: PathBuf,
    design_config: rsdm_core::domain::DesignConfig,
    limiter: Arc<MemoryLoginAttemptLimiter>,
}

struct LockSurface {
    output: wl_output::WlOutput,
    output_name: String,
    surface: SessionLockSurface,
    viewport: Option<WpViewport>,
    _fractional_scale: Option<WpFractionalScaleV1>,
    scale_120: u32,
    preferred_scale_120: Option<u32>,
    width: u32,
    height: u32,
    buffer: Option<Buffer>,
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    pool: SlotPool,
    compositor_state: CompositorState,
    viewporter: Option<WpViewporter>,
    fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
    session_lock_state: SessionLockState,
    session_lock: Option<SessionLock>,
    surfaces: Vec<LockSurface>,
    primary_output: Option<wl_output::WlOutput>,
    powered_off_outputs: Vec<String>,
    off_fallback_warned: bool,
    keyboards: Vec<(wl_seat::WlSeat, wl_keyboard::WlKeyboard)>,
    model: LockModel,
    authentication: Option<crate::auth::AuthenticationJob>,
    ctx: LockContext,
    menu: Menu,
    menu_enabled: bool,
    pending: Option<LockPending>,
    needs_redraw: bool,
    animation_started: Instant,
    last_animation_frame: u64,
    exit: bool,
    unlocked: bool,
    lock_state: Option<rsdm_infra::lock_control::LockStateGuard>,
}

pub fn run(config: &AppConfig, config_path: &Path) -> Result<()> {
    rsdm_infra::unix::install_emergency_unlock_handler();
    let context = build_context(config, config_path)?;
    let mut menu = Menu::new();
    menu.show_wallpaper_controls(context.wallpaper.is_some());
    menu.enable_lock_controls(LockMenuSettings {
        size: config.lock.size,
        secondary_output: config.lock.secondary_output,
    });

    let connection = Connection::connect_to_env().context("connecting to the Wayland display")?;
    let (globals, mut queue) =
        registry_queue_init(&connection).context("initializing the Wayland registry")?;
    let queue_handle = queue.handle();
    let shm = Shm::bind(&globals, &queue_handle).context("compositor lacks wl_shm")?;
    let pool = SlotPool::new(256 * 256 * 4, &shm).context("creating the shm pool")?;
    let viewporter = globals.bind(&queue_handle, 1..=1, ()).ok();
    let fractional_scale_manager = globals.bind(&queue_handle, 1..=1, ()).ok();

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &queue_handle),
        seat_state: SeatState::new(&globals, &queue_handle),
        shm,
        pool,
        compositor_state: CompositorState::bind(&globals, &queue_handle)
            .context("compositor lacks wl_compositor")?,
        viewporter,
        fractional_scale_manager,
        session_lock_state: SessionLockState::new(&globals, &queue_handle),
        session_lock: None,
        surfaces: Vec::new(),
        primary_output: None,
        powered_off_outputs: Vec::new(),
        off_fallback_warned: false,
        keyboards: Vec::new(),
        model: LockModel::new(config.lock.design.password_mode),
        authentication: None,
        ctx: context,
        menu,
        menu_enabled: config.lock.design.menu,
        pending: None,
        needs_redraw: false,
        animation_started: Instant::now(),
        last_animation_frame: 0,
        exit: false,
        unlocked: false,
        lock_state: None,
    };

    let lock = app
        .session_lock_state
        .lock(&queue_handle)
        .map_err(|_| anyhow::anyhow!("compositor does not support ext-session-lock-v1"))?;
    app.session_lock = Some(lock);
    // A compositor may wait for our first frames before confirming the lock.
    app.choose_primary_output();
    app.create_surfaces(&queue_handle);
    run_event_loop(&connection, &mut queue, &mut app)?;

    if !app.unlocked {
        anyhow::bail!("compositor ended or refused the Wayland session lock");
    }
    connection
        .roundtrip()
        .context("confirming session unlock with the compositor")?;
    Ok(())
}

fn run_event_loop(
    connection: &Connection,
    queue: &mut smithay_client_toolkit::reexports::client::EventQueue<App>,
    app: &mut App,
) -> Result<()> {
    while !app.exit {
        queue
            .dispatch_pending(app)
            .context("Wayland dispatch failed")?;
        app.process_authentication();
        if rsdm_infra::unix::emergency_unlock_requested() && app.lock_state.is_some() {
            app.unlock();
        }
        if app.needs_redraw {
            app.draw_all();
            app.last_animation_frame = app.animation_frame();
            app.needs_redraw = false;
        }
        if app.exit {
            break;
        }

        let timeout = if app.authentication.is_some() {
            app.animation_poll_timeout_ms().min(50)
        } else {
            app.animation_poll_timeout_ms()
        };
        poll_wayland(queue, timeout)?;
        if app.ctx.design.background.is_animated()
            && app.animation_frame() != app.last_animation_frame
        {
            app.needs_redraw = true;
        }
    }
    connection.flush().context("flushing Wayland requests")
}

fn poll_wayland<State>(
    queue: &mut smithay_client_toolkit::reexports::client::EventQueue<State>,
    timeout_ms: i32,
) -> Result<()>
where
    State: 'static,
{
    queue.flush().context("flushing Wayland requests")?;
    let Some(read_guard) = queue.prepare_read() else {
        return Ok(());
    };
    let mut pollfd = libc::pollfd {
        fd: read_guard.connection_fd().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };

    // SAFETY: pollfd points to one initialized element for the duration of poll.
    let result = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
    if result > 0 {
        read_guard.read().context("reading Wayland events")?;
    } else {
        drop(read_guard);
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error).context("polling the Wayland socket");
            }
        }
    }
    Ok(())
}

fn build_context(config: &AppConfig, config_path: &Path) -> Result<LockContext> {
    let design = Design::from_config(&config.lock.design);
    let wallpaper = config
        .lock
        .design
        .wallpaper
        .as_deref()
        .and_then(load_wallpaper);
    let username = util::current_username().context("could not determine the current lock user")?;

    Ok(LockContext {
        font: crate::render::Font::load(&config.dm.tty.path),
        design,
        wallpaper,
        username,
        hibernate_available: input::hibernate_available(),
        pam_service: config.lock.pam_service.clone(),
        primary_output: config.lock.primary_output.clone(),
        secondary_output: config.lock.secondary_output,
        config_path: config_path.to_path_buf(),
        design_config: config.lock.design.clone(),
        limiter: Arc::new(MemoryLoginAttemptLimiter::new(
            config.security.max_failed_attempts,
            Duration::from_millis(config.security.failure_delay_ms),
        )),
    })
}

fn load_wallpaper(path: &str) -> Option<Wallpaper> {
    match Wallpaper::load(Path::new(path)) {
        Ok(wallpaper) => Some(wallpaper),
        Err(error) => {
            tracing::warn!(%path, %error, "failed to load wallpaper");
            None
        }
    }
}

//! Wayland session locker using the shared rsdm UI.

mod auth;
mod model;
mod render;
mod util;
mod wayland;

use std::path::Path;

use anyhow::Context;
use rsdm_core::domain::{AppConfig, Rgb};
use rsdm_ui::{Design, LockScene, Menu, Surface, banner};

use crate::{
    model::LockModel,
    render::{Canvas, FbSurface, Font, Wallpaper, resolve_zoom, tint},
};

/// Lock every output until the seated user authenticates.
pub fn run(config: &AppConfig, config_path: &Path) -> anyhow::Result<()> {
    if !config.lock.enable {
        anyhow::bail!("lock screen is disabled by [lock].enable = false");
    }
    tracing::info!(
        pam_service = %config.lock.pam_service,
        theme = ?config.lock.design.theme,
        wallpaper = ?config.lock.design.wallpaper,
        "starting lock screen"
    );
    wayland::run(config, config_path)
}

/// Paint the wallpaper/effect base and report whether a wallpaper was present.
pub(crate) fn compose_lock_base(
    canvas: &mut Canvas,
    design: &Design,
    wallpaper: Option<&Wallpaper>,
    font: &Font,
    zoom: u32,
    frame: u64,
) -> bool {
    let has_wallpaper = wallpaper.is_some();
    if let Some(wallpaper) = wallpaper {
        wallpaper.cover_into(canvas);
        tint(canvas, Rgb::BLACK, design.dim_alpha());
    }
    if design.background.is_animated() {
        let opacity = if has_wallpaper {
            design.background_opacity_byte()
        } else {
            255
        };
        let p = design.palette();
        let mut surface = FbSurface::with_opacity(canvas, font, zoom, opacity);
        let area = surface.area();
        rsdm_ui::render_lock_background(&mut surface, design, area, p, frame);
    }
    has_wallpaper
}

/// Render a representative lock screen to a PNG without a compositor.
pub fn preview_png(
    config: &AppConfig,
    width: u32,
    height: u32,
    animation_frame: u64,
    path: &Path,
) -> anyhow::Result<()> {
    tracing::debug!(
        width,
        height,
        path = %path.display(),
        "rendering lock screen preview"
    );
    let design = Design::from_config(&config.lock.design);
    let p = design.palette();
    let font = Font::load(&config.dm.tty.path);
    let zoom = resolve_zoom(config.lock.size);
    let mut canvas = Canvas::try_new(width, height, p.bg_base.argb(0xff))
        .context("allocating preview framebuffer")?;

    let wallpaper = config
        .lock
        .design
        .wallpaper
        .as_deref()
        .and_then(|path| Wallpaper::load(Path::new(path)).ok());
    let wallpaper_present = compose_lock_base(
        &mut canvas,
        &design,
        wallpaper.as_ref(),
        &font,
        zoom,
        animation_frame,
    );

    let mut model = LockModel::new(config.lock.design.password_mode);
    for ch in "hunter2".chars() {
        model.push(ch);
    }
    model.set_error("Incorrect password");
    let username = util::current_username().unwrap_or_else(|| "user".to_string());
    let clock = design.show_clock.then(banner::clock_text);
    let hostname = design.show_hostname.then(banner::hostname);
    let scene = LockScene {
        title: banner::title_lines(&design, None),
        hostname,
        clock,
        username: &username,
        password_preview: model.password_preview(),
        authentication_active: false,
        message: model.message(),
        message_is_error: model.message_is_error(),
        pending: None,
        hibernate_available: false,
    };
    {
        let mut surface = FbSurface::new(&mut canvas, &font, zoom);
        let area = surface.area();
        rsdm_ui::render_lock_content(
            &mut surface,
            &design,
            &scene,
            &Menu::new(),
            p,
            area,
            wallpaper_present,
        );
    }
    image::save_buffer(
        path,
        &canvas.to_rgba8(),
        width,
        height,
        image::ColorType::Rgba8,
    )
    .with_context(|| format!("writing preview to {}", path.display()))
}

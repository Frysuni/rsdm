use rsdm_core::domain::SecondaryOutput;
use rsdm_ui::{LockScene, Surface, banner};
use smithay_client_toolkit::reexports::client::QueueHandle;

use super::{ANIMATION_FRAME, App, LockSurface, buffers::BufferGeometry};
use crate::render::{Canvas, FbSurface, resolve_zoom};

impl App {
    pub(super) fn create_surfaces(&mut self, qh: &QueueHandle<Self>) {
        let Some(lock) = self.session_lock.clone() else {
            return;
        };
        for output in self.output_state.outputs() {
            if self.surfaces.iter().any(|surface| surface.output == output) {
                continue;
            }

            let surface = self.compositor_state.create_surface(qh);
            let viewport = self
                .viewporter
                .as_ref()
                .zip(self.fractional_scale_manager.as_ref())
                .map(|(viewporter, _)| viewporter.get_viewport(&surface, qh, ()));
            let fractional_scale = self
                .fractional_scale_manager
                .as_ref()
                .zip(viewport.as_ref())
                .map(|(manager, _)| manager.get_fractional_scale(&surface, qh, surface.clone()));
            let lock_surface = lock.create_lock_surface(surface, &output, qh);

            self.surfaces.push(LockSurface {
                output_name: self.output_name(&output),
                scale_120: self.output_scale_120(&output),
                preferred_scale_120: None,
                output,
                surface: lock_surface,
                viewport,
                _fractional_scale: fractional_scale,
                width: 0,
                height: 0,
                buffers: Vec::new(),
                canvas: Canvas::default(),
                black_geometry: None,
                wallpaper_cache: None,
            });
        }
    }

    pub(super) fn draw_all(&mut self) {
        for index in 0..self.surfaces.len() {
            self.draw(index);
        }
    }

    fn draw(&mut self, index: usize) {
        let Some(surface) = self.surfaces.get(index) else {
            return;
        };
        if surface.width == 0 || surface.height == 0 {
            return;
        }

        let logical_width = surface.width;
        let logical_height = surface.height;
        let fractional = surface.viewport.is_some();
        let primary = self.primary_output.as_ref() == Some(&surface.output);
        let output_name = surface.output_name.clone();
        let (width, height, buffer_scale) = buffer_dimensions(
            logical_width,
            logical_height,
            surface.scale_120,
            surface.preferred_scale_120,
            fractional,
        );
        if !valid_buffer_dimensions(width, height) {
            tracing::error!(output = %output_name, width, height, "invalid lock buffer dimensions");
            return;
        }

        let geometry = BufferGeometry {
            width, height, logical_width, logical_height, scale: buffer_scale,
        };
        let content = if primary {
            OutputContent::Primary
        } else if matches!(self.ctx.secondary_output, SecondaryOutput::Black | SecondaryOutput::Off) {
            OutputContent::Black
        } else {
            OutputContent::Background
        };
        if content == OutputContent::Black && surface.black_geometry == Some(geometry) { return; }
        let Some(buffer) = self.prepare_buffer(index, width, height) else { return; };
        let mut canvas = std::mem::take(&mut self.surfaces[index].canvas);
        match self.paint_frame(index, &mut canvas, geometry, content) {
            Ok(()) => {
                if self.commit_buffer(index, buffer, &canvas, geometry) {
                    self.surfaces[index].black_geometry = (content == OutputContent::Black).then_some(geometry);
                }
            }
            Err(error) => tracing::error!(%error, output = %output_name, "failed to allocate lock framebuffer"),
        }
        self.surfaces[index].canvas = canvas;
    }

    fn paint_frame(
        &mut self, index: usize, canvas: &mut Canvas, geometry: BufferGeometry, content: OutputContent,
    ) -> anyhow::Result<()> {
        let palette = self.ctx.design.palette();
        let initial = if content == OutputContent::Black {
            0xff00_0000
        } else {
            palette.bg_base.argb(0xff)
        };
        canvas.resize(geometry.width, geometry.height, initial)?;
        if content == OutputContent::Black { return Ok(()); }

        let zoom = resolve_zoom(self.menu.lock_settings().and_then(|settings| settings.size));
        let frame = self.animation_frame();
        let wallpaper_present = crate::compose_lock_base(
            canvas, &self.ctx.design, self.ctx.wallpaper.as_ref(), &self.ctx.font,
            zoom, frame, &mut self.surfaces[index].wallpaper_cache,
        );
        if content == OutputContent::Primary {
            self.draw_primary(canvas, zoom, palette, wallpaper_present);
        }
        Ok(())
    }

    fn draw_primary(
        &self,
        canvas: &mut Canvas,
        zoom: u32,
        palette: rsdm_core::domain::Palette,
        wallpaper_present: bool,
    ) {
        let scene = LockScene {
            title: if self.authentication.is_some() {
                Vec::new()
            } else {
                banner::title_lines(&self.ctx.design, None)
            },
            hostname: self.ctx.design.show_hostname.then(banner::hostname),
            clock: self.ctx.design.show_clock.then(banner::clock_text),
            username: &self.ctx.username,
            password_preview: self.model.password_preview(),
            authentication_active: self.authentication.is_some(),
            message: self.model.message(),
            message_is_error: self.model.message_is_error(),
            pending: self.pending,
            hibernate_available: self.ctx.hibernate_available,
            suspend_available: self.ctx.suspend_available,
        };
        let mut surface = FbSurface::new(canvas, &self.ctx.font, zoom);
        let area = surface.area();
        rsdm_ui::render_lock_content(
            &mut surface,
            &self.ctx.design,
            &scene,
            &self.menu,
            palette,
            area,
            wallpaper_present,
        );
    }

    pub(super) fn animation_frame(&self) -> u64 {
        let tick_ms = ANIMATION_FRAME.as_millis().max(1);
        (self.animation_started.elapsed().as_millis() / tick_ms) as u64
    }

    pub(super) fn animation_poll_timeout_ms(&self) -> i32 {
        if !self.ctx.design.background.is_animated() {
            return 250;
        }
        let tick_ms = ANIMATION_FRAME.as_millis().max(1);
        let elapsed_ms = self.animation_started.elapsed().as_millis();
        (tick_ms - elapsed_ms % tick_ms).clamp(1, 250) as i32
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputContent {
    Primary,
    Background,
    Black,
}

fn buffer_dimensions(
    width: u32,
    height: u32,
    scale_120: u32,
    preferred_scale_120: Option<u32>,
    fractional: bool,
) -> (u32, u32, i32) {
    if fractional {
        let scale_120 = preferred_scale_120.unwrap_or(scale_120).max(1);
        return (
            scaled_dimension(width, scale_120),
            scaled_dimension(height, scale_120),
            1,
        );
    }
    let scale = scale_120.div_ceil(120).max(1);
    (
        width.saturating_mul(scale),
        height.saturating_mul(scale),
        scale as i32,
    )
}

fn valid_buffer_dimensions(width: u32, height: u32) -> bool {
    if width == 0 || height == 0 || width > (i32::MAX / 4) as u32 || height > i32::MAX as u32 {
        return false;
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .is_some_and(|bytes| bytes <= crate::render::canvas::MAX_CANVAS_BYTES)
}

fn scaled_dimension(logical: u32, scale_120: u32) -> u32 {
    let scaled = (logical as u64)
        .saturating_mul(scale_120.max(1) as u64)
        .saturating_add(60)
        / 120;
    scaled.min(u32::MAX as u64) as u32
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;

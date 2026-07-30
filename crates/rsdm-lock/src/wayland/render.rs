use rsdm_core::domain::SecondaryOutput;
use rsdm_ui::{LockScene, Surface, banner};
use smithay_client_toolkit::reexports::client::{QueueHandle, protocol::wl_shm};

use super::{ANIMATION_FRAME, App, LockSurface};
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
                output,
                surface: lock_surface,
                viewport,
                _fractional_scale: fractional_scale,
                width: 0,
                height: 0,
                buffer: None,
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
        let scale_120 = surface.scale_120.max(120);
        let fractional = surface.viewport.is_some();
        let primary = self.primary_output.as_ref() == Some(&surface.output);
        let output_name = surface.output_name.clone();
        let (width, height, buffer_scale) =
            buffer_dimensions(logical_width, logical_height, scale_120, fractional);
        if !valid_buffer_dimensions(width, height) {
            tracing::error!(output = %output_name, width, height, "invalid lock buffer dimensions");
            return;
        }

        let palette = self.ctx.design.palette();
        let black_secondary = !primary
            && matches!(
                self.ctx.secondary_output,
                SecondaryOutput::Black | SecondaryOutput::Off
            );
        let initial = if black_secondary {
            0xff00_0000
        } else {
            palette.bg_base.argb(0xff)
        };
        let mut canvas = match Canvas::try_new(width, height, initial) {
            Ok(canvas) => canvas,
            Err(error) => {
                tracing::error!(%error, output = %output_name, "failed to allocate lock framebuffer");
                return;
            }
        };

        let zoom = resolve_zoom(
            height,
            self.menu.lock_settings().and_then(|settings| settings.size),
        );
        let wallpaper_present = !black_secondary
            && crate::compose_lock_base(
                &mut canvas,
                &self.ctx.design,
                self.ctx.wallpaper.as_ref(),
                zoom,
                self.animation_frame(),
            );
        if primary {
            self.draw_primary(&mut canvas, zoom, palette, wallpaper_present);
        }
        self.commit_buffer(
            index,
            canvas,
            BufferGeometry {
                width,
                height,
                logical_width,
                logical_height,
                scale: buffer_scale,
            },
        );
    }

    fn draw_primary(
        &self,
        canvas: &mut Canvas,
        zoom: u32,
        palette: rsdm_core::domain::Palette,
        wallpaper_present: bool,
    ) {
        let scene = LockScene {
            title: banner::title_lines(&self.ctx.design, None),
            hostname: self.ctx.design.show_hostname.then(banner::hostname),
            clock: self.ctx.design.show_clock.then(banner::clock_text),
            username: &self.ctx.username,
            password_preview: self.model.password_preview(),
            message: self.model.message(),
            message_is_error: self.model.message_is_error(),
            pending: self.pending,
            hibernate_available: self.ctx.hibernate_available,
        };
        let mut surface = FbSurface::new(canvas, zoom);
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

    fn commit_buffer(&mut self, index: usize, canvas: Canvas, geometry: BufferGeometry) {
        let buffer = self.pool.create_buffer(
            geometry.width as i32,
            geometry.height as i32,
            geometry.width as i32 * 4,
            wl_shm::Format::Argb8888,
        );
        let Ok((buffer, slot)) = buffer else {
            tracing::error!("failed to allocate shm buffer for the locker");
            return;
        };
        slot[..canvas.as_bytes().len()].copy_from_slice(canvas.as_bytes());

        let surface = &mut self.surfaces[index];
        let wl_surface = surface.surface.wl_surface().clone();
        wl_surface.set_buffer_scale(geometry.scale);
        if let Some(viewport) = &surface.viewport {
            viewport.set_destination(
                geometry.logical_width as i32,
                geometry.logical_height as i32,
            );
        }
        if let Err(error) = buffer.attach_to(&wl_surface) {
            tracing::debug!(%error, "lock shm buffer is active; skipping redraw");
            return;
        }
        wl_surface.damage_buffer(0, 0, geometry.width as i32, geometry.height as i32);
        wl_surface.commit();
        surface.buffer = Some(buffer);
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

struct BufferGeometry {
    width: u32,
    height: u32,
    logical_width: u32,
    logical_height: u32,
    scale: i32,
}

fn buffer_dimensions(width: u32, height: u32, scale_120: u32, fractional: bool) -> (u32, u32, i32) {
    if fractional {
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
mod tests {
    use super::*;

    #[test]
    fn fractional_dimensions_round_half_up() {
        assert_eq!(scaled_dimension(1920, 180), 2880);
        assert_eq!(scaled_dimension(721, 180), 1082);
        assert_eq!(scaled_dimension(100, 120), 100);
    }

    #[test]
    fn buffer_dimensions_bound_stride_and_memory() {
        assert!(valid_buffer_dimensions(7680, 4320));
        assert!(!valid_buffer_dimensions(0, 1080));
        assert!(!valid_buffer_dimensions(i32::MAX as u32, 1));
        assert!(!valid_buffer_dimensions(16_384, 16_384));
    }
}

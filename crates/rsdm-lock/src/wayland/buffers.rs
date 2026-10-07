//! Reuse at most two SHM buffers per output, respecting compositor ownership.

use smithay_client_toolkit::{
    reexports::client::protocol::wl_shm,
    shm::slot::{Buffer, CreateBufferError, SlotPool},
};

use super::App;
use crate::render::Canvas;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct BufferGeometry {
    pub width: u32,
    pub height: u32,
    pub logical_width: u32,
    pub logical_height: u32,
    pub scale: i32,
}

impl App {
    pub(super) fn prepare_buffer(&mut self, index: usize, width: u32, height: u32) -> Option<usize> {
        match reusable_buffer(&mut self.surfaces[index].buffers, &mut self.pool, width, height) {
            Ok(Some(buffer)) => Some(buffer),
            Ok(None) => {
                self.needs_redraw = true;
                None
            }
            Err(error) => {
                tracing::error!(%error, "failed to allocate shm buffer for the locker");
                None
            }
        }
    }

    pub(super) fn commit_buffer(
        &mut self, index: usize, buffer_index: usize, canvas: &Canvas, geometry: BufferGeometry,
    ) -> bool {
        let surface = &self.surfaces[index];
        let buffer = &surface.buffers[buffer_index];
        let Some(slot) = buffer.canvas(&mut self.pool) else {
            self.needs_redraw = true;
            return false;
        };
        slot.copy_from_slice(canvas.as_bytes());

        let wl_surface = surface.surface.wl_surface();
        wl_surface.set_buffer_scale(geometry.scale);
        if let Some(viewport) = &surface.viewport {
            viewport.set_destination(geometry.logical_width as i32, geometry.logical_height as i32);
        }
        if let Err(error) = buffer.attach_to(wl_surface) {
            tracing::debug!(%error, "lock shm buffer is active; retrying redraw");
            self.needs_redraw = true;
            return false;
        }
        wl_surface.damage_buffer(0, 0, geometry.width as i32, geometry.height as i32);
        wl_surface.commit();
        true
    }
}

fn reusable_buffer(
    buffers: &mut Vec<Buffer>, pool: &mut SlotPool, width: u32, height: u32,
) -> Result<Option<usize>, CreateBufferError> {
    let stride = width as i32 * 4;
    let matches = |buffer: &Buffer| buffer.stride() == stride && buffer.height() == height as i32;
    // Keep old-size active buffers until Release: dropping them and allocating
    // replacements on every Configure could grow the pool without a bound.
    buffers.retain(|buffer| matches(buffer) || buffer.canvas(pool).is_none());
    if let Some(index) = buffers.iter().position(|buffer| matches(buffer) && buffer.canvas(pool).is_some()) {
        return Ok(Some(index));
    }
    if buffers.len() >= 2 { return Ok(None); }

    let (buffer, _) = pool.create_buffer(width as i32, height as i32, stride, wl_shm::Format::Argb8888)?;
    buffers.push(buffer);
    Ok(Some(buffers.len() - 1))
}

#[cfg(test)]
#[path = "buffer_tests.rs"]
mod tests;

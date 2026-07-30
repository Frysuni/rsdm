//! Boilerplate Wayland protocol wiring for [`App`].
//!
//! These handlers carry no locker logic - they only forward registry, seat,
//! output, compositor and shm events into `App`'s state (acquiring the keyboard,
//! covering new outputs, requesting a redraw). The handlers with real behaviour
//! (keyboard input and the session-lock lifecycle) live in the parent module.

use smithay_client_toolkit::{
    compositor::CompositorHandler,
    delegate_compositor, delegate_keyboard, delegate_output, delegate_registry, delegate_seat,
    delegate_session_lock, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::client::{
        Connection, QueueHandle,
        protocol::{wl_output, wl_seat, wl_surface},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{Capability, SeatHandler, SeatState},
    shm::{Shm, ShmHandler},
};

use super::App;

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seat_state.get_keyboard(qh, &seat, None) {
                Ok(keyboard) => {
                    tracing::debug!("keyboard acquired for lock screen");
                    self.keyboard = Some(keyboard);
                }
                Err(error) => tracing::error!(%error, "failed to acquire keyboard"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            tracing::debug!("keyboard removed from lock screen");
            keyboard.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: wl_output::WlOutput) {
        // An output appeared after the lock engaged: cover it too.
        tracing::debug!("Wayland output added");
        if self.session_lock.is_some() {
            self.choose_primary_output();
            self.power_off_secondary_outputs();
            self.create_surfaces(qh);
            self.needs_redraw = true;
        }
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.choose_primary_output();
        self.power_off_secondary_outputs();
        self.needs_redraw = true;
    }

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        tracing::debug!("Wayland output removed");
        self.surfaces.retain(|surface| surface.output != output);
        if self.primary_output.as_ref() == Some(&output) {
            self.restore_secondary_outputs();
            self.choose_primary_output();
            self.needs_redraw = true;
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if let Some(entry) = self
            .surfaces
            .iter_mut()
            .find(|entry| entry.surface.wl_surface() == surface)
        {
            // Fractional-scale will refine this value (for example from 240 to
            // 180 for 1.5x). Until then, the integer scale is still a crisp and
            // protocol-correct fallback.
            entry.scale_120 = factor.max(1) as u32 * 120;
            self.needs_redraw = true;
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        // Animation is timer-driven at ~15 fps by the main loop. No frame
        // callback is requested during normal rendering, but tolerate a stale
        // callback from a surface created by an older runtime state.
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_shm!(App);
delegate_session_lock!(App);
delegate_registry!(App);

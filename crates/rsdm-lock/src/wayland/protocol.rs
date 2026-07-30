use smithay_client_toolkit::{
    reexports::client::{Connection, Dispatch, QueueHandle, delegate_noop, protocol::wl_surface},
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockSurface, SessionLockSurfaceConfigure,
    },
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

use super::App;

impl SessionLockHandler for App {
    fn locked(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, _lock: SessionLock) {
        tracing::info!("Wayland compositor confirmed session lock");
        match rsdm_infra::lock_control::register_lock() {
            Ok(guard) => self.lock_state = Some(guard),
            Err(error) => tracing::error!(%error, "lock runtime state is unavailable"),
        }
        self.choose_primary_output();
        self.power_off_secondary_outputs();
        self.create_surfaces(qh);
        self.needs_redraw = true;
    }

    fn finished(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _lock: SessionLock) {
        tracing::warn!("Wayland compositor ended or refused the session lock");
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _serial: u32,
    ) {
        let (width, height) = configure.new_size;
        if let Some(entry) = self
            .surfaces
            .iter_mut()
            .find(|entry| entry.surface.wl_surface() == surface.wl_surface())
        {
            entry.width = width;
            entry.height = height;
        }
        self.needs_redraw = true;
    }
}

impl Dispatch<WpFractionalScaleV1, wl_surface::WlSurface> for App {
    fn event(
        state: &mut Self,
        _proxy: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        surface: &wl_surface::WlSurface,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let wp_fractional_scale_v1::Event::PreferredScale { scale } = event else {
            return;
        };
        if let Some(entry) = state
            .surfaces
            .iter_mut()
            .find(|entry| entry.surface.wl_surface() == surface)
        {
            entry.scale_120 = scale.max(1);
            state.needs_redraw = true;
        }
    }
}

delegate_noop!(App: ignore WpViewporter);
delegate_noop!(App: ignore WpViewport);
delegate_noop!(App: ignore WpFractionalScaleManagerV1);

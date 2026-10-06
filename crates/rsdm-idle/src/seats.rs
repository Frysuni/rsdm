//! Idle notifications for every seat in the current Wayland connection.

use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_noop,
    globals::GlobalListContents,
    protocol::{wl_registry, wl_seat},
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

use super::IdleApp;

pub(super) struct SeatNotification {
    name: u32,
    seat: wl_seat::WlSeat,
    notification: ExtIdleNotificationV1,
    idle: bool,
}

impl IdleApp {
    pub(super) fn add_seat(
        &mut self,
        registry: &wl_registry::WlRegistry,
        name: u32,
        version: u32,
        qh: &QueueHandle<Self>,
    ) {
        if version == 0 || self.seats.iter().any(|seat| seat.name == name) {
            return;
        }
        let seat = registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(9), qh, ());
        let notification = if self.config.ignore_inhibitors && self.notifier.version() >= 2 {
            self.notifier.get_input_idle_notification(self.timeout_ms, &seat, qh, name)
        } else {
            self.notifier.get_idle_notification(self.timeout_ms, &seat, qh, name)
        };
        self.seats.push(SeatNotification { name, seat, notification, idle: false });
        tracing::debug!(name, "monitoring Wayland seat activity");
    }

    fn remove_seat(&mut self, name: u32) {
        let Some(index) = self.seats.iter().position(|seat| seat.name == name) else {
            return;
        };
        let entry = self.seats.swap_remove(index);
        entry.notification.destroy();
        if entry.seat.version() >= 5 {
            entry.seat.release();
        }
        self.lock_if_all_seats_idle();
    }

    fn lock_if_all_seats_idle(&self) {
        if !self.seats.is_empty() && self.seats.iter().all(|seat| seat.idle) {
            self.start_lock_cycle();
        }
    }
}

impl Dispatch<ExtIdleNotificationV1, u32> for IdleApp {
    fn event(
        state: &mut Self,
        _proxy: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        name: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(seat) = state.seats.iter_mut().find(|seat| seat.name == *name) else {
            return;
        };
        match event {
            ext_idle_notification_v1::Event::Idled => seat.idle = true,
            ext_idle_notification_v1::Event::Resumed => seat.idle = false,
            _ => return,
        }
        state.lock_if_all_seats_idle();
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for IdleApp {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == "wl_seat" => {
                state.add_seat(registry, name, version, qh);
            }
            wl_registry::Event::GlobalRemove { name } => state.remove_seat(name),
            _ => {}
        }
    }
}

delegate_noop!(IdleApp: ignore wl_seat::WlSeat);
delegate_noop!(IdleApp: ExtIdleNotifierV1);

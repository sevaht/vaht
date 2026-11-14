//! Idle: `ext_idle_notifier_v1` and `zwp_idle_inhibit_manager_v1`.

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::idle_inhibit::IdleInhibitHandler;
use smithay::wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState};

use crate::Compositor;

impl IdleNotifierHandler for Compositor {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.protocols.idle_notifier
    }
}

impl IdleInhibitHandler for Compositor {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.add(surface);
        self.refresh_idle_inhibition();
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.remove(&surface);
        self.refresh_idle_inhibition();
    }
}

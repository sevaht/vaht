//! Locking the session: `ext_session_lock_manager_v1`.

use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::wayland::session_lock::{
    LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
};

use crate::Compositor;
use crate::outputs::Outputs;

impl SessionLockHandler for Compositor {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.protocols.session_lock
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        self.menus.close();
        // Nothing an X program held stays held behind the lock.
        self.release_all_emulated();
        self.session_lock.lock(confirmation);
        self.settle();
        self.schedule_render();
    }

    fn unlock(&mut self) {
        self.session_lock.unlock();
        self.settle();
        self.schedule_render();
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let Some(output) = Output::from_resource(&output) else {
            return;
        };
        let area = Outputs::geometry(&output);
        self.session_lock.add_surface(surface, output, area);
        self.settle();
        self.schedule_render();
    }
}

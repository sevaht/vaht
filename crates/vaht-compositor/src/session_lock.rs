//! Locking the session: `ext_session_lock_v1`, which swaylock and other
//! lockers use.
//!
//! While the session is locked, each output shows only the locker's
//! surface for it (black until there is one), and only those surfaces get
//! the pointer and the keyboard; bindings rest, but for the terminal
//! switch. The locker hears that the session is locked only once a frame
//! showing nothing of the session has been drawn. If the locker dies, the
//! session stays locked: what it hid stays hidden until a locker unlocks.

use smithay::output::Output;
use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::session_lock::{LockSurface, SessionLocker};

/// Whether the session is locked.
#[derive(Debug, Default)]
enum LockState {
    /// Open: the session shows.
    #[default]
    Unlocked,
    /// Locked; the locker hears so once a frame shows it.
    Confirming(SessionLocker),
    /// Locked, and the locker knows (or died: the session stays locked).
    Locked,
}

/// The session lock, and each output's lock surface.
#[derive(Debug, Default)]
pub(crate) struct SessionLock {
    /// Whether it is locked.
    state: LockState,
    /// Each output's lock surface, while locked.
    surfaces: Vec<(Output, LockSurface)>,
}

impl SessionLock {
    /// Returns whether the session is locked.
    pub(crate) const fn is_locked(&self) -> bool {
        !matches!(self.state, LockState::Unlocked)
    }

    /// Locks the session for `locker`, who hears so after the next frame.
    pub(crate) fn lock(&mut self, locker: SessionLocker) {
        tracing::info!("session locked");
        self.state = LockState::Confirming(locker);
        self.surfaces.clear();
    }

    /// Unlocks the session: its locker asked.
    pub(crate) fn unlock(&mut self) {
        tracing::info!("session unlocked");
        self.state = LockState::Unlocked;
        self.surfaces.clear();
    }

    /// Takes `surface`, the locker's for `output` (which covers `area`),
    /// and asks the locker to fill the output.
    pub(crate) fn add_surface(
        &mut self,
        surface: LockSurface,
        output: Output,
        area: Rectangle<i32, Logical>,
    ) {
        let size = (
            u32::try_from(area.size.w).unwrap_or_default(),
            u32::try_from(area.size.h).unwrap_or_default(),
        );
        surface.with_pending_state(|state| state.size = Some(size.into()));
        surface.send_configure();
        self.surfaces.retain(|(known, _)| *known != output);
        self.surfaces.push((output, surface));
    }

    /// Tells the locker the session is locked, if it has not heard: a frame
    /// showing nothing of the session was just drawn.
    pub(crate) fn confirm(&mut self) {
        self.state = match std::mem::take(&mut self.state) {
            LockState::Confirming(locker) => {
                locker.lock();
                LockState::Locked
            }
            unchanged @ (LockState::Unlocked | LockState::Locked) => unchanged,
        };
    }

    /// Returns `output`'s lock surface, if the locker gave it one that
    /// lives.
    pub(crate) fn surface_on(&self, output: &Output) -> Option<&WlSurface> {
        self.surfaces
            .iter()
            .find(|(known, surface)| known == output && surface.wl_surface().is_alive())
            .map(|(_, surface)| surface.wl_surface())
    }

    /// Returns every lock surface that lives, with its output.
    pub(crate) fn surfaces(&self) -> impl Iterator<Item = (&Output, &WlSurface)> {
        self.surfaces
            .iter()
            .filter(|(_, surface)| surface.wl_surface().is_alive())
            .map(|(output, surface)| (output, surface.wl_surface()))
    }
}

//! The client's side of every window: the protocol objects behind the
//! window manager's windows.
//!
//! [`Toplevels`] maps each [`WindowId`] the window manager issued to the
//! [`Toplevel`] it stands for. The two are added and removed together, by
//! the compositor, when a client creates or destroys a toplevel; the leak
//! tests check that none outlives its window.

mod toplevel;

use std::collections::HashMap;

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

pub(crate) use self::toplevel::Toplevel;
use crate::resources::{ResourceKind, ResourceLedger, Tracked};
use crate::window_manager::WindowId;

/// Every window's toplevel, by window.
#[derive(Debug, Default)]
pub(crate) struct Toplevels {
    /// Each toplevel, with its count, by the window it stands for.
    by_window: HashMap<WindowId, (Toplevel, Tracked)>,
}

impl Toplevels {
    /// Returns the toplevel of `window`, if it has one.
    pub(crate) fn get(&self, window: WindowId) -> Option<&Toplevel> {
        self.by_window.get(&window).map(|(toplevel, _)| toplevel)
    }

    /// Returns the toplevel of `window` to change, if it has one.
    pub(crate) fn get_mut(&mut self, window: WindowId) -> Option<&mut Toplevel> {
        self.by_window
            .get_mut(&window)
            .map(|(toplevel, _)| toplevel)
    }

    /// Returns the window whose root surface is `surface`.
    pub(crate) fn window_of(&self, surface: &WlSurface) -> Option<WindowId> {
        self.by_window
            .iter()
            .find(|(_, (toplevel, _))| toplevel.has_surface(surface))
            .map(|(window, _)| *window)
    }

    /// Records that `toplevel` stands for `window`.
    pub(crate) fn insert(&mut self, window: WindowId, toplevel: Toplevel, ledger: &ResourceLedger) {
        self.by_window
            .insert(window, (toplevel, ledger.track(ResourceKind::Toplevel)));
    }

    /// Forgets the toplevel whose root surface is `surface`, which is gone;
    /// returns the window it stood for.
    pub(crate) fn remove(&mut self, surface: &WlSurface) -> Option<WindowId> {
        let window = self.window_of(surface)?;
        self.by_window.remove(&window);
        Some(window)
    }
}

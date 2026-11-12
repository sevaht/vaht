//! Every managed window, and the order they are stacked in.
//!
//! [`Windows`] is the only owner of [`Window`]s. Other subsystems hold
//! [`WindowId`]s and look windows up here; a closed window's id simply finds
//! nothing. The [`Stack`] is the single record of stacking order, read
//! top-down for hit-testing and bottom-up for drawing, so what is clicked is
//! always what is seen.

mod stack;

use std::collections::HashMap;

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};

use self::stack::Stack;
use crate::resources::ResourceLedger;
use crate::window::{Toplevel, Window, WindowId};

/// All managed windows, by id, and their stacking order.
#[derive(Debug, Default)]
pub(crate) struct Windows {
    /// Every window, by id.
    by_id: HashMap<WindowId, Window>,
    /// Their stacking order.
    stack: Stack,
    /// The id the next window gets.
    next_id: u64,
}

impl Windows {
    /// Returns the window with `id`, if it still exists.
    pub(crate) fn get(&self, id: WindowId) -> Option<&Window> {
        self.by_id.get(&id)
    }

    /// Returns the window whose root surface is `surface`.
    pub(crate) fn id_of(&self, surface: &WlSurface) -> Option<WindowId> {
        self.by_id
            .values()
            .find(|window| window.toplevel().has_surface(surface))
            .map(Window::id)
    }

    /// Returns how many windows there are.
    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Returns the topmost window.
    pub(crate) fn top(&self) -> Option<WindowId> {
        self.stack.top()
    }

    /// Returns the windows from top to bottom, the order composition and
    /// hit-testing use.
    pub(crate) fn top_down(&self) -> impl Iterator<Item = &Window> + '_ {
        self.stack.top_down().filter_map(|id| self.by_id.get(&id))
    }

    /// Returns every window, in no particular order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Window> + '_ {
        self.by_id.values()
    }

    /// Returns the topmost window under `point`, the client surface there,
    /// and that surface's origin, all in global coordinates.
    pub(crate) fn surface_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(WindowId, WlSurface, Point<f64, Logical>)> {
        self.stack.top_down().find_map(|id| {
            let window = self.by_id.get(&id)?;
            let (surface, origin) = window.surface_under(point)?;
            Some((id, surface, origin))
        })
    }

    /// Adds a window on top of the others, its content at `location`.
    pub(crate) fn insert(
        &mut self,
        toplevel: Toplevel,
        location: Point<i32, Logical>,
        ledger: &ResourceLedger,
    ) -> WindowId {
        self.next_id += 1;
        let id = WindowId::new(self.next_id);
        self.by_id
            .insert(id, Window::new(id, toplevel, location, ledger));
        self.stack.push(id);
        id
    }

    /// Removes and returns the window with `id`.
    pub(crate) fn remove(&mut self, id: WindowId) -> Option<Window> {
        self.stack.remove(id);
        self.by_id.remove(&id)
    }

    /// Moves the window with `id` above all others; returns whether it exists.
    pub(crate) fn raise(&mut self, id: WindowId) -> bool {
        self.stack.raise(id)
    }
}

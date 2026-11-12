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
use vaht_ui::theme::Metrics;

use self::stack::Stack;
use crate::resources::ResourceLedger;
use crate::window::{FramePart, Toplevel, Window, WindowId};

/// What of a window is under a point.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WindowTarget {
    /// One of the client's surfaces.
    Surface {
        /// The window it belongs to.
        window: WindowId,
        /// The surface.
        surface: WlSurface,
        /// The surface's origin, in global coordinates.
        origin: Point<f64, Logical>,
    },
    /// Part of the frame vaht draws around it.
    Frame {
        /// The window the frame surrounds.
        window: WindowId,
        /// Which part.
        part: FramePart,
    },
}

impl WindowTarget {
    /// Returns the window the target belongs to.
    pub(crate) const fn window(&self) -> WindowId {
        match self {
            Self::Surface { window, .. } | Self::Frame { window, .. } => *window,
        }
    }
}

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

    /// Returns the window with `id`, to change it.
    pub(crate) fn get_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.by_id.get_mut(&id)
    }

    /// Returns every window, in no particular order, to change them.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Window> + '_ {
        self.by_id.values_mut()
    }

    /// Returns what is under `point` (global coordinates): the topmost
    /// window's client surface or frame there, with frames sized by
    /// `metrics`.
    pub(crate) fn target_at(
        &self,
        point: Point<f64, Logical>,
        metrics: Metrics,
    ) -> Option<WindowTarget> {
        self.stack.top_down().find_map(|id| {
            let window = self.by_id.get(&id)?;
            if let Some((surface, origin)) = window.surface_under(point) {
                return Some(WindowTarget::Surface {
                    window: id,
                    surface,
                    origin,
                });
            }
            let part = window.frame_geometry(metrics).part_at(point)?;
            Some(WindowTarget::Frame { window: id, part })
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

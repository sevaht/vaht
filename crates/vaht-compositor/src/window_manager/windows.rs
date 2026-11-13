//! Every managed window, and the order they are stacked in.
//!
//! [`Windows`] is the only owner of [`Window`]s. Everything else holds
//! [`WindowId`]s and looks windows up here; a closed window's id simply finds
//! nothing. The [`Stack`] is the single record of stacking order, read
//! top-down for hit-testing and bottom-up for drawing, so what is clicked is
//! always what is seen.

mod stack;

use std::collections::HashMap;

use self::stack::Stack;
pub(crate) use self::stack::StackLayer;
use super::window::{Frame, Window, WindowId};
use super::workspaces::Workspace;
use crate::resources::ResourceLedger;

/// All managed windows, by id, and their stacking order.
#[derive(Debug, Default)]
pub(crate) struct Windows {
    /// Every window, by id.
    by_id: HashMap<WindowId, Window>,
    /// Their stacking order.
    stack: Stack,
    /// The id the last window got.
    last_id: u64,
}

impl Windows {
    /// Returns the window with `id`, if it still exists.
    pub(crate) fn get(&self, id: WindowId) -> Option<&Window> {
        self.by_id.get(&id)
    }

    /// Returns how many windows there are.
    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Returns the windows from top to bottom.
    pub(crate) fn top_down(&self) -> impl Iterator<Item = &Window> + '_ {
        self.stack.top_down().filter_map(|id| self.by_id.get(&id))
    }

    /// Returns the windows shown while `workspace` is, from top to bottom:
    /// the order composition and hit-testing use.
    pub(crate) fn shown_top_down(
        &self,
        workspace: Workspace,
    ) -> impl Iterator<Item = &Window> + '_ {
        self.top_down()
            .filter(move |window| window.is_shown_on(workspace))
    }

    /// Returns the window with `id`, to change it.
    pub(crate) fn get_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.by_id.get_mut(&id)
    }

    /// Returns every window, in no particular order, to change them.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Window> + '_ {
        self.by_id.values_mut()
    }

    /// Adds a window with `frame` on `workspace`, on top of the others, and
    /// returns its id.
    pub(crate) fn insert(
        &mut self,
        frame: Frame,
        workspace: Workspace,
        ledger: &ResourceLedger,
    ) -> WindowId {
        self.last_id += 1;
        let id = WindowId::new(self.last_id);
        self.by_id
            .insert(id, Window::new(id, frame, workspace, ledger));
        self.stack.push(id);
        id
    }

    /// Removes and returns the window with `id`.
    pub(crate) fn remove(&mut self, id: WindowId) -> Option<Window> {
        self.stack.remove(id);
        self.by_id.remove(&id)
    }

    /// Moves the window with `id` below all others; returns whether it exists.
    pub(crate) fn lower(&mut self, id: WindowId) -> bool {
        self.stack.lower(id)
    }

    /// Moves the window with `id` above all others; returns whether it exists.
    pub(crate) fn raise(&mut self, id: WindowId) -> bool {
        self.stack.raise(id)
    }

    /// Moves the window with `id` to the top of `layer`; returns whether it
    /// exists.
    pub(crate) fn set_layer(&mut self, id: WindowId, layer: StackLayer) -> bool {
        self.stack.set_layer(id, layer)
    }

    /// Returns the layer of the window with `id`.
    pub(crate) fn layer_of(&self, id: WindowId) -> Option<StackLayer> {
        self.stack.layer_of(id)
    }
}

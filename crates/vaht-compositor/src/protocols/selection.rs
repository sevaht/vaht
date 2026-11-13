//! The primary selection (`zwp_primary_selection_device_manager_v1`): what
//! was last selected, pasted with the middle button.
//!
//! Its focus follows keyboard focus, set with the clipboard's in
//! `Compositor::sync_keyboard_focus`, so a client that is gone is never kept alive
//! by it.

use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState,
};

use crate::Compositor;

impl PrimarySelectionHandler for Compositor {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.protocols.primary_selection
    }
}

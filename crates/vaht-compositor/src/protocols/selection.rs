//! The selections: the clipboard and the primary selection (what was last
//! selected, pasted with the middle button), shared with X programs and
//! with clipboard managers.
//!
//! Their focus follows keyboard focus, set with the clipboard's in
//! `Compositor::sync_keyboard_focus`, so a client that is gone is never
//! kept alive by it. Clipboard managers (`wl-copy`, `wl-paste --watch`,
//! clipboard histories) read and set both through data control, in its
//! `ext` and `wlr` forms, without a window or the keyboard.
//!
//! Xwayland's window manager stands in for X programs: what a Wayland
//! client or a clipboard manager selects is offered to X programs, and what
//! an X program selects is held for Wayland clients under vaht's own name
//! (the selection's data is `()`), which [`SelectionHandler::send_selection`]
//! fetches from the X program.

use std::os::fd::OwnedFd;

use smithay::input::Seat;
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::selection::{ext_data_control, wlr_data_control};

use crate::Compositor;

impl SelectionHandler for Compositor {
    /// Nothing: the only selections vaht holds are X programs'.
    type SelectionUserData = ();

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if let Some(xwayland) = &mut self.xwayland {
            xwayland.offer_selection(ty, source.map(|source| source.mime_types()));
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        _user_data: &Self::SelectionUserData,
    ) {
        if let Some(xwayland) = &mut self.xwayland {
            xwayland.give_selection(ty, mime_type, fd);
        }
    }
}

impl PrimarySelectionHandler for Compositor {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.protocols.primary_selection
    }
}

impl ext_data_control::DataControlHandler for Compositor {
    fn data_control_state(&mut self) -> &mut ext_data_control::DataControlState {
        &mut self.protocols.data_control.0
    }
}

impl wlr_data_control::DataControlHandler for Compositor {
    fn data_control_state(&mut self) -> &mut wlr_data_control::DataControlState {
        &mut self.protocols.data_control.1
    }
}

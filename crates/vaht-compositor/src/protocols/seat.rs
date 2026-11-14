//! Input devices, the clipboard and drag and drop: `wl_seat` and
//! `wl_data_device_manager`.

use smithay::input::dnd::{DnDGrab, DndGrabHandler, GrabType, Source};
use smithay::input::keyboard::LedState;
use smithay::input::pointer::{CursorImageStatus, Focus};
use smithay::input::tablet::TabletSeatHandler;

use crate::backend::Backend;
use crate::focus_target::FocusTarget;
use smithay::input::pointer::PointerHandle;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Serial;
use smithay::utils::{Logical, Point};
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
};
use smithay::wayland::pointer_constraints::{
    ConstraintRemove, PointerConstraint, PointerConstraintsHandler,
};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus,
};

use crate::Compositor;

impl SeatHandler for Compositor {
    type KeyboardFocus = FocusTarget;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.protocols.seat
    }

    fn led_state_changed(&mut self, _seat: &Seat<Self>, leds: LedState) {
        if let Backend::Drm(backend) = &mut self.backend {
            backend.set_keyboard_leds(leds);
        }
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor.set_status(image);
        self.schedule_render();
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&FocusTarget>) {
        // The clipboard follows keyboard focus: only the focused client may
        // read it.
        let client = focused
            .and_then(FocusTarget::surface)
            .and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, seat, client);
    }
}

/// Games lock the pointer (it stays put; they read relative motion) or
/// confine it to their window. A constraint holds while its surface is
/// under the pointer and has the keyboard.
impl PointerConstraintsHandler for Compositor {
    fn new_constraint(&mut self, surface: &WlSurface, _pointer: &PointerHandle<Self>) {
        self.activate_pointer_constraint(surface);
    }

    fn remove_constraint(
        &mut self,
        surface: &WlSurface,
        _pointer: &PointerHandle<Self>,
        removed: ConstraintRemove,
    ) {
        // A lock ending puts the pointer where the game last said it is.
        if let ConstraintRemove::Destroyed(PointerConstraint::Locked(_)) = removed {
            self.end_pointer_lock(surface);
        }
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        _pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        self.pointer_lock_hint = Some((surface.clone(), location));
    }
}

impl SelectionHandler for Compositor {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Compositor {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.protocols.data_device
    }
}

impl DndGrabHandler for Compositor {}

impl WaylandDndGrabHandler for Compositor {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        _icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        grab_type: GrabType,
    ) {
        let pointer_grab = match grab_type {
            GrabType::Pointer => seat
                .get_pointer()
                .and_then(|pointer| Some((pointer.grab_start_data()?, pointer))),
            // Touch drags arrive with touch input, which vaht does not take yet.
            GrabType::Touch => None,
        };
        match pointer_grab {
            Some((start_data, pointer)) => {
                let grab = DnDGrab::new_pointer(&self.display_handle, start_data, source, seat);
                pointer.set_grab(self, grab, serial, Focus::Keep);
            }
            None => source.cancel(),
        }
    }
}

// Tablets come with libinput; until then no tool has focus, but the cursor
// shape protocol asks for this.
impl TabletSeatHandler for Compositor {
    type ToolFocus = WlSurface;
}

/// Games, virtual machines and remote desktops ask for every key, Super's
/// chords included, while they have the keyboard. vaht grants it at once;
/// the terminal switch keys stay vaht's, so the user is never shut in.
impl KeyboardShortcutsInhibitHandler for Compositor {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.protocols.keyboard_shortcuts_inhibit
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        tracing::debug!("a window takes every key while it has the keyboard");
        inhibitor.activate();
    }
}

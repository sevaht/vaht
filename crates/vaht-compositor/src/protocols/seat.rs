//! Input devices, the clipboard and drag and drop: `wl_seat` and
//! `wl_data_device_manager`.

use smithay::input::dnd::{DnDGrab, DndGrabHandler, DndTarget, GrabType, Source};
use smithay::input::keyboard::LedState;
use smithay::input::pointer::{CursorImageStatus, Focus};
use smithay::input::tablet::TabletSeatHandler;

use crate::backend::Backend;
use crate::focus_target::FocusTarget;
use smithay::input::pointer::PointerHandle;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource;
use smithay::reexports::wayland_server::protocol::wl_pointer::WlPointer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Serial;
use smithay::utils::{Logical, Point};
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
};
use smithay::wayland::pointer_constraints::{
    ConstraintRemove, PointerConstraint, PointerConstraintsHandler,
};
use smithay::wayland::pointer_warp::PointerWarpHandler;
use smithay::wayland::selection::data_device::{
    DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus,
};

use crate::Compositor;
use crate::protocols::toplevel_drag::{HeldCancel, HeldSource};
use std::sync::Arc;

/// A client moving the pointer within its own surface, as Xwayland does
/// for an X program's warp.
impl PointerWarpHandler for Compositor {
    fn warp_pointer(
        &mut self,
        surface: WlSurface,
        _pointer: WlPointer,
        pos: Point<f64, Logical>,
        serial: Serial,
    ) {
        Self::warp_pointer(self, &surface, pos, serial);
    }
}

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

impl DataDeviceHandler for Compositor {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.protocols.data_device
    }
}

impl DndGrabHandler for Compositor {
    /// The window dragged along, if one was, stays where it was let go.
    fn dropped(
        &mut self,
        _target: Option<DndTarget<'_, Self>>,
        _validated: bool,
        _seat: Seat<Self>,
        _location: Point<f64, Logical>,
    ) {
        // A window dragged along is let go, even where nothing accepted the
        // drop: its program is told the drag was dropped, not cancelled.
        let is_window_dragged = self.dragged_window().is_some();
        if let Some(held) = self.held_drag.take() {
            held.end(is_window_dragged);
        }
        self.dragged_source = None;
    }

    fn cancelled(&mut self, _seat: Seat<Self>, _location: Point<f64, Logical>) {
        if let Some(held) = self.held_drag.take() {
            held.end(false);
        }
        self.dragged_source = None;
    }
}

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
                // A program's own drag carries its wl_data_source, which a
                // window may be attached to (a tab dragged out).
                self.dragged_source = (&source as &dyn std::any::Any)
                    .downcast_ref::<WlDataSource>()
                    .cloned();
                let held = Arc::new(HeldCancel::new(source));
                self.held_drag = Some(Arc::<HeldCancel<S>>::clone(&held));
                let grab =
                    DnDGrab::new_pointer(&self.display_handle, start_data, HeldSource(held), seat);
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

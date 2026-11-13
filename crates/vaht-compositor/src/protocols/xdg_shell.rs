//! Application windows and their popups: `xdg_wm_base`.

use smithay::desktop::{PopupKind, find_popup_root_surface, get_popup_toplevel_coords};
use smithay::input::Seat;
use smithay::input::pointer::GrabStartData;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::Compositor;
use crate::window::{
    Area, Axes, HorizontalEdge, Placement, ResizeEdges, VerticalEdge, WindowDrag, WindowId,
};

impl XdgShellHandler for Compositor {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.protocols.xdg_shell
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.add_window(surface);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_window(surface.wl_surface());
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not track a popup"
            );
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    // Popup grabs (menus that close on an outside click) come with input
    // grabs in general.
    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.request_placement(
            &surface,
            |placement| !placement.is_fully_maximized(),
            |placement, current| placement.with_maximized_toggled(Axes::Both, current),
        );
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.request_placement(
            &surface,
            Placement::is_fully_maximized,
            |placement, current| placement.with_maximized_toggled(Axes::Both, current),
        );
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        self.request_placement(
            &surface,
            |placement| !placement.is_fullscreen(),
            Placement::with_fullscreen_toggled,
        );
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.request_placement(
            &surface,
            Placement::is_fullscreen,
            Placement::with_fullscreen_toggled,
        );
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        let window = self.windows.id_of(surface.wl_surface());
        self.set_minimized(window, true);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: WlSeat, serial: Serial) {
        let Some((window, start)) = self.client_drag(&surface, &seat, serial) else {
            return;
        };
        self.start_window_grab(window, WindowDrag::Move, start, serial);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let Some(edges) = resize_edges(edges) else {
            return;
        };
        if let Some((window, start)) = self.client_drag(&surface, &seat, serial) {
            self.start_window_grab(window, WindowDrag::Resize { edges }, start, serial);
        }
    }
}

/// Returns the edges an xdg-shell resize request names, or `None` for none.
const fn resize_edges(edge: xdg_toplevel::ResizeEdge) -> Option<ResizeEdges> {
    let (horizontal, vertical) = match edge {
        xdg_toplevel::ResizeEdge::Top => (HorizontalEdge::None, VerticalEdge::Top),
        xdg_toplevel::ResizeEdge::Bottom => (HorizontalEdge::None, VerticalEdge::Bottom),
        xdg_toplevel::ResizeEdge::Left => (HorizontalEdge::Left, VerticalEdge::None),
        xdg_toplevel::ResizeEdge::Right => (HorizontalEdge::Right, VerticalEdge::None),
        xdg_toplevel::ResizeEdge::TopLeft => (HorizontalEdge::Left, VerticalEdge::Top),
        xdg_toplevel::ResizeEdge::TopRight => (HorizontalEdge::Right, VerticalEdge::Top),
        xdg_toplevel::ResizeEdge::BottomLeft => (HorizontalEdge::Left, VerticalEdge::Bottom),
        xdg_toplevel::ResizeEdge::BottomRight => (HorizontalEdge::Right, VerticalEdge::Bottom),
        // Values from protocol versions newer than vaht's mean nothing to it.
        xdg_toplevel::ResizeEdge::None | _ => return None,
    };
    Some(ResizeEdges {
        horizontal,
        vertical,
    })
}

impl Compositor {
    /// Returns the window a client asks to drag, and where the drag starts,
    /// if the request is genuine: the pointer button behind `serial` is
    /// still held, on a surface of the same client. Anything else is a
    /// stale or forged request, and is ignored.
    fn client_drag(
        &self,
        surface: &ToplevelSurface,
        seat: &WlSeat,
        serial: Serial,
    ) -> Option<(WindowId, GrabStartData<Self>)> {
        let pointer = Seat::<Self>::from_resource(seat)?.get_pointer()?;
        if !pointer.has_grab(serial) {
            return None;
        }
        let start = pointer.grab_start_data()?;
        let (focus, _) = start.focus.as_ref()?;
        if !focus.id().same_client_as(&surface.wl_surface().id()) {
            return None;
        }
        Some((self.windows.id_of(surface.wl_surface())?, start))
    }

    /// Places the window of `surface` as `change` says, if `applies` to its
    /// placement; the client always hears back, as xdg-shell requires.
    fn request_placement(
        &mut self,
        surface: &ToplevelSurface,
        applies: impl FnOnce(&Placement) -> bool,
        change: impl FnOnce(Placement, Area) -> Placement,
    ) {
        let window = self.windows.id_of(surface.wl_surface());
        let changes = window
            .and_then(|id| self.windows.get(id))
            .is_some_and(|target| applies(target.placement()));
        if changes {
            self.change_placement(window, change);
        } else if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
    }

    /// Keeps a popup within its window's output, flipping or sliding it as its
    /// positioner allows.
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self
            .windows
            .id_of(&root)
            .and_then(|id| self.windows.get(id))
        else {
            return;
        };
        // The positioner works relative to the popup's parent.
        let mut target = self.backend.output_geometry();
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window.location();
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

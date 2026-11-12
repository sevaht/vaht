//! Application windows and their popups: `xdg_wm_base`.

use smithay::desktop::{PopupKind, find_popup_root_surface, get_popup_toplevel_coords};
use smithay::input::Seat;
use smithay::input::pointer::GrabStartData;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::Compositor;
use crate::grab::GrabKind;
use crate::window::{HorizontalEdge, ResizeEdges, VerticalEdge, Window, WindowId};

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

    fn move_request(&mut self, surface: ToplevelSurface, seat: WlSeat, serial: Serial) {
        let Some((window, start)) = self.client_drag(&surface, &seat, serial) else {
            return;
        };
        let Some(initial_location) = self.windows.get(window).map(Window::location) else {
            return;
        };
        self.start_window_grab(window, GrabKind::Move { initial_location }, start, serial);
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
            self.start_window_grab(window, GrabKind::Resize { edges }, start, serial);
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

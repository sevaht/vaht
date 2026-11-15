//! Application windows and their popups: `xdg_wm_base`.

use smithay::desktop::PopupKind;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::dialog::{ToplevelDialogHint, XdgDialogHandler};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::xdg_toplevel_icon::XdgToplevelIconHandler;

use crate::Compositor;
use crate::window_manager::{ClientRequest, HorizontalEdge, ResizeEdges, VerticalEdge, WindowDrag};

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
        self.honor_client_request(&surface, ClientRequest::Maximize);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.honor_client_request(&surface, ClientRequest::Unmaximize);
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        self.honor_client_request(&surface, ClientRequest::Fullscreen);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.honor_client_request(&surface, ClientRequest::Unfullscreen);
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        self.honor_client_request(&surface, ClientRequest::Minimize);
    }

    fn parent_changed(&mut self, surface: ToplevelSurface) {
        self.follow_family(surface.wl_surface());
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: WlSeat, serial: Serial) {
        self.start_requested_drag(&surface, &seat, serial, WindowDrag::Move);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        if let Some(edges) = resize_edges(edges) {
            self.start_requested_drag(&surface, &seat, serial, WindowDrag::Resize { edges });
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

/// A dialog's modal hint (`xdg_wm_dialog_v1`) decides whether it keeps the
/// focus from its parent.
impl XdgDialogHandler for Compositor {
    fn dialog_hint_changed(&mut self, toplevel: ToplevelSurface, _hint: ToplevelDialogHint) {
        self.follow_family(toplevel.wl_surface());
    }
}

/// A window's icon is part of its next commit, where the window's
/// `Toplevel` reads it; nothing needs doing when it is set.
impl XdgToplevelIconHandler for Compositor {}

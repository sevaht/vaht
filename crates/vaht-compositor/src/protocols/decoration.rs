//! Who draws a window's frame: `zxdg_decoration_manager_v1`, and KDE's
//! older `org_kde_kwin_server_decoration_manager`, which some Qt and KDE
//! programs ask instead.
//!
//! vaht always draws frames itself, so it answers every request with
//! server-side decoration. A client that asks to draw its own is told no;
//! without that, terminals and toolkits would draw a second titlebar inside
//! vaht's.

use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::{Mode as KdeMode, OrgKdeKwinServerDecoration};
use smithay::reexports::wayland_server::WEnum;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::shell::kde::decoration::{KdeDecorationHandler, KdeDecorationState};
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;

use crate::Compositor;

impl XdgDecorationHandler for Compositor {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        use_server_side_decoration(&toplevel);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: Mode) {
        use_server_side_decoration(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        use_server_side_decoration(&toplevel);
    }

    // The client gave up negotiating; vaht keeps drawing the frame.
    fn decoration_destroyed(&mut self, _toplevel: ToplevelSurface) {}
}

impl KdeDecorationHandler for Compositor {
    fn kde_decoration_state(&self) -> &KdeDecorationState {
        &self.protocols.kde_decoration
    }

    /// Any mode but the server's is answered with it; the server's is not
    /// answered again, so a client acknowledging it starts no loop.
    fn request_mode(
        &mut self,
        _surface: &WlSurface,
        decoration: &OrgKdeKwinServerDecoration,
        mode: WEnum<KdeMode>,
    ) {
        if mode != WEnum::Value(KdeMode::Server) {
            decoration.mode(KdeMode::Server);
        }
    }
}

/// Tells `toplevel` that vaht draws its frame; once its first configure has
/// gone out, the change is sent right away.
fn use_server_side_decoration(toplevel: &ToplevelSurface) {
    toplevel.with_pending_state(|state| {
        state.decoration_mode = Some(Mode::ServerSide);
    });
    if toplevel.is_initial_configure_sent() {
        toplevel.send_pending_configure();
    }
}

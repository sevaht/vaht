//! Who draws a window's frame: `zxdg_decoration_manager_v1`, and KDE's
//! older `org_kde_kwin_server_decoration_manager`, which some Qt and KDE
//! programs ask instead.
//!
//! vaht draws a window's frame unless its program asks to draw its own, as
//! fluxland did: through xdg-decoration a program that asks for client-side
//! decoration gets it and no frame from vaht (Chromium and Firefox draw
//! their tab strips so), and one with no preference gets vaht's. Through
//! KDE's older protocol, which Qt and KDE programs use to ask for the
//! server's frame, every request is answered with vaht's.

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
        set_decoration_mode(&toplevel, Mode::ServerSide);
    }

    /// The program's choice is honored, as fluxland honored it: one that
    /// draws its own frame (Chromium's tab strip, unless its "use system
    /// title bar" is on) gets none from vaht, rather than two.
    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        set_decoration_mode(&toplevel, mode);
    }

    /// No preference: vaht draws the frame.
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        set_decoration_mode(&toplevel, Mode::ServerSide);
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

/// Tells `toplevel` who draws its frame, `mode`; once its first configure
/// has gone out, the change is sent right away. The window manager learns
/// it when the client commits after acknowledging it.
fn set_decoration_mode(toplevel: &ToplevelSurface, mode: Mode) {
    toplevel.with_pending_state(|state| {
        state.decoration_mode = Some(mode);
    });
    if toplevel.is_initial_configure_sent() {
        toplevel.send_pending_configure();
    }
}

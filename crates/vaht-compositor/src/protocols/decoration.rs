//! Who draws a window's frame: `zxdg_decoration_manager_v1`, and KDE's
//! older `org_kde_kwin_server_decoration_manager`, which some Qt and KDE
//! programs ask instead.
//!
//! vaht draws a window's frame unless its program asks to draw its own, as
//! fluxland did: through xdg-decoration a program that asks for client-side
//! decoration gets it and no frame from vaht (Chromium and Firefox draw
//! their tab strips so), and one with no preference gets vaht's. A program
//! that makes no decoration object at all draws its own frame, as the
//! protocol has it (Wine makes one only for windows with a caption, so a
//! frameless Electron window gets none from vaht). Through
//! KDE's older protocol, which Qt and KDE programs (and GTK 3, which
//! draws no frame of its own when it is told the server's) use to ask for
//! the server's frame, every request is answered with vaht's, and the window
//! is one vaht frames whether or not it also made an xdg-decoration object.

use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::{Mode as KdeMode, OrgKdeKwinServerDecoration};
use smithay::reexports::wayland_server::WEnum;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::shell::kde::decoration::{KdeDecorationHandler, KdeDecorationState};
use smithay::utils::IsAlive;
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

    /// The client gave up negotiating: with no decoration object a window
    /// draws its own frame (Wine does this when a window loses its
    /// caption).
    fn decoration_destroyed(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = None;
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}

impl KdeDecorationHandler for Compositor {
    fn kde_decoration_state(&self) -> &KdeDecorationState {
        &self.protocols.kde_decoration
    }

    /// A window that makes one is framed by vaht, unless it never gets the
    /// chance to say otherwise.
    fn new_decoration(&mut self, surface: &WlSurface, _decoration: &OrgKdeKwinServerDecoration) {
        self.frame_by_vaht(surface);
    }

    /// Any mode but the server's is answered with it; the server's is not
    /// answered again, so a client acknowledging it starts no loop.
    fn request_mode(
        &mut self,
        surface: &WlSurface,
        decoration: &OrgKdeKwinServerDecoration,
        mode: WEnum<KdeMode>,
    ) {
        self.frame_by_vaht(surface);
        if mode != WEnum::Value(KdeMode::Server) {
            decoration.mode(KdeMode::Server);
        }
    }

    /// The program let go of the decoration: with no object it draws its own
    /// frame, as after an xdg-decoration object is destroyed.
    fn release(&mut self, _decoration: &OrgKdeKwinServerDecoration, surface: &WlSurface) {
        self.protocols
            .kde_decorated
            .retain(|known| known != surface);
        if let Some(toplevel) = self.toplevel_of(surface) {
            toplevel.with_pending_state(|state| {
                state.decoration_mode = None;
            });
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
        }
    }
}

impl Compositor {
    /// Notes that the program of `surface` asked for the server's frame, and
    /// tells its window, if it has one yet; one made after is told as it is
    /// made (`window_made`).
    fn frame_by_vaht(&mut self, surface: &WlSurface) {
        self.protocols.kde_decorated.retain(IsAlive::alive);
        if !self.protocols.kde_decorated.contains(surface) {
            self.protocols.kde_decorated.push(surface.clone());
        }
        if let Some(toplevel) = self.toplevel_of(surface) {
            set_decoration_mode(&toplevel, Mode::ServerSide);
        }
    }

    /// Returns the window of `surface`, if it is one.
    fn toplevel_of(&self, surface: &WlSurface) -> Option<ToplevelSurface> {
        self.protocols
            .xdg_shell
            .toplevel_surfaces()
            .iter()
            .find(|toplevel| toplevel.wl_surface() == surface)
            .cloned()
    }

    /// Tells a window just made whose program already asked for the
    /// server's frame, KDE's way, that it has it.
    pub(super) fn window_made(&self, toplevel: &ToplevelSurface) {
        if self.protocols.kde_decorated.contains(toplevel.wl_surface()) {
            set_decoration_mode(toplevel, Mode::ServerSide);
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

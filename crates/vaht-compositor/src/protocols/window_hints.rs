//! What windows say about themselves beyond `xdg_toplevel`: a bell rung
//! (`xdg_system_bell_v1`), a tag and a description
//! (`xdg_toplevel_tag_manager_v1`), and a parent in another program
//! (`zxdg_exporter_v2` and `zxdg_importer_v2`, which portal dialogs use to
//! sit over the window that opened them).
//!
//! Smithay keeps a window's tag and description with it, and sets the
//! parent an imported handle names; vaht has no use for a bell yet, so it
//! is only logged.

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::xdg_foreign::{XdgForeignHandler, XdgForeignState};
use smithay::wayland::xdg_system_bell::XdgSystemBellHandler;
use smithay::wayland::xdg_toplevel_tag::XdgToplevelTagHandler;

use crate::Compositor;

impl XdgSystemBellHandler for Compositor {
    fn ring(&mut self, surface: Option<WlSurface>) {
        tracing::debug!(?surface, "a program rang the bell");
    }
}

impl XdgToplevelTagHandler for Compositor {}

impl XdgForeignHandler for Compositor {
    fn xdg_foreign_state(&mut self) -> &mut XdgForeignState {
        &mut self.protocols.xdg_foreign
    }
}

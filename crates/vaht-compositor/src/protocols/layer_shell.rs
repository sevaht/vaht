//! Layer surfaces: `zwlr_layer_shell_v1`.

use smithay::desktop::{LayerSurface as MappedLayerSurface, PopupKind};
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface, WlrLayerShellHandler, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::PopupSurface;

use crate::Compositor;

impl WlrLayerShellHandler for Compositor {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.protocols.layer_shell
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        _output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // Every layer surface goes on the one output until there are more.
        let output = self.backend.output().clone();
        self.layers.map(
            MappedLayerSurface::new(surface, namespace),
            &output,
            &self.ledger,
        );
    }

    fn new_popup(&mut self, _parent: LayerSurface, popup: PopupSurface) {
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(popup)) {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not track a popup"
            );
        }
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        let output = self.backend.output().clone();
        self.layers.unmap(surface.wl_surface(), &output);
        self.refresh_layer_keyboard();
        self.schedule_render();
    }
}

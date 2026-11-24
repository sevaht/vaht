//! Layer surfaces (`zwlr_layer_shell_v1`): wallpapers, panels, notifications
//! and launchers that sit in a layer below or above the windows.
//!
//! Smithay's [`LayerMap`] (one per output, kept in the output's data)
//! arranges them by their anchors and exclusive zones; what they leave free
//! is the output's usable area, where windows are maximized and tiled.
//! [`Layers`] keeps each mapped layer surface counted with its output;
//! [`OutputLayers`] answers what is where on one output, and
//! [`LayerMaps`] on all of them. The scene draws the background and bottom
//! layers below the windows, and the top and overlay layers above them.

use std::sync::MutexGuard;

use smithay::desktop::{LayerMap, LayerSurface, WindowSurfaceType, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer, LayerSurfaceData};

use crate::outputs::Outputs;
use crate::resources::{ResourceKind, ResourceLedger, Tracked};

/// The layers drawn above the windows, topmost first.
pub(crate) const ABOVE_WINDOWS: [Layer; 2] = [Layer::Overlay, Layer::Top];

/// The layers drawn above a fullscreen window, which covers the rest.
pub(crate) const ABOVE_FULLSCREEN: [Layer; 1] = [Layer::Overlay];

/// Every layer, topmost first.
pub(crate) const ALL_LAYERS: [Layer; 4] =
    [Layer::Overlay, Layer::Top, Layer::Bottom, Layer::Background];

/// The layers drawn below the windows, topmost first.
pub(crate) const BELOW_WINDOWS: [Layer; 2] = [Layer::Bottom, Layer::Background];

/// The mapped layer surfaces, counted while they live.
#[derive(Debug, Default)]
pub(crate) struct Layers {
    /// Each mapped layer surface, with the output it is on and its count.
    mapped: Vec<(LayerSurface, Output, Tracked)>,
    /// The layer surface holding the keyboard, if one does.
    keyboard: Option<WlSurface>,
}

impl Layers {
    /// Returns the layer surface holding the keyboard, if one does.
    pub(crate) const fn keyboard(&self) -> Option<&WlSurface> {
        self.keyboard.as_ref()
    }

    /// Gives the keyboard to a layer surface of any output that wants it to
    /// itself (a launcher, a lock screen). Without one, a layer surface
    /// that took the keyboard on a click keeps it while it still takes
    /// keys; otherwise the windows have it back.
    pub(crate) fn refresh_keyboard(&mut self, maps: LayerMaps<'_>) {
        let kept = self
            .keyboard
            .take()
            .filter(|surface| self.takes_keyboard_on_click(surface));
        self.keyboard = maps.exclusive_keyboard_surface().or(kept);
    }

    /// Gives the keyboard to the layer surface `surface`, clicked, if it
    /// takes the keyboard on a click.
    pub(crate) fn click(&mut self, surface: &WlSurface) {
        if self.takes_keyboard_on_click(surface) {
            self.keyboard = Some(surface.clone());
        }
    }

    /// Returns the layer surface whose root is `surface`, if it is one.
    pub(crate) fn find(&self, surface: &WlSurface) -> Option<&LayerSurface> {
        self.mapped
            .iter()
            .map(|(layer, _, _)| layer)
            .find(|layer| layer.wl_surface() == surface)
    }

    /// Returns every mapped layer surface, with the output it is on.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&LayerSurface, &Output)> {
        self.mapped.iter().map(|(layer, output, _)| (layer, output))
    }

    /// Maps `surface` on `output`; it is configured when it first commits.
    pub(crate) fn map(&mut self, surface: LayerSurface, output: &Output, ledger: &ResourceLedger) {
        let mut map = layer_map_for_output(output);
        if let Err(error) = map.map_layer(&surface) {
            tracing::warn!(%error, "could not map a layer surface");
            return;
        }
        tracing::debug!(
            namespace = surface.namespace(),
            output = output.name(),
            "layer surface mapped"
        );
        drop(map);
        self.mapped.push((
            surface,
            output.clone(),
            ledger.track(ResourceKind::LayerSurface),
        ));
    }

    /// Unmaps the layer surface whose root is `surface`, which is gone.
    pub(crate) fn unmap(&mut self, surface: &WlSurface) {
        self.mapped.retain(|(layer, output, _)| {
            let is_gone = layer.wl_surface() == surface;
            if is_gone {
                let mut map = layer_map_for_output(output);
                map.unmap_layer(layer);
                map.arrange();
            }
            !is_gone
        });
        if self.keyboard.as_ref() == Some(surface) {
            self.keyboard = None;
        }
    }

    /// Closes the layer surfaces on outputs no longer among `outputs` (one
    /// unplugged or turned off): each client is told, as the protocol
    /// asks, and may open its surface again on another output.
    pub(crate) fn close_off(&mut self, outputs: &Outputs) {
        let keyboard = &mut self.keyboard;
        self.mapped.retain(|(layer, output, _)| {
            let is_on = outputs.iter().any(|on| on == output);
            if !is_on {
                tracing::debug!(
                    namespace = layer.namespace(),
                    output = output.name(),
                    "layer surface closed: its output is gone"
                );
                layer.layer_surface().send_close();
                layer_map_for_output(output).unmap_layer(layer);
                if keyboard.as_ref() == Some(layer.wl_surface()) {
                    *keyboard = None;
                }
            }
            is_on
        });
    }

    /// Returns whether the layer surface `surface` takes the keyboard when
    /// clicked.
    fn takes_keyboard_on_click(&self, surface: &WlSurface) -> bool {
        self.find(surface).is_some_and(|layer| {
            layer.cached_state().keyboard_interactivity != KeyboardInteractivity::None
        })
    }
}

/// The layer surfaces of one output, which covers `whole` in global
/// coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OutputLayers<'output> {
    /// The output; its data holds the layer map.
    output: &'output Output,
    /// Where the output is.
    whole: Rectangle<i32, Logical>,
}

impl<'output> OutputLayers<'output> {
    /// The layer surfaces of `output`, which covers `whole`.
    pub(crate) const fn new(output: &'output Output, whole: Rectangle<i32, Logical>) -> Self {
        Self { output, whole }
    }

    /// Returns the area the layer surfaces leave free, in global
    /// coordinates.
    pub(crate) fn usable_area(self) -> Rectangle<i32, Logical> {
        let mut usable = layer_map_for_output(self.output).non_exclusive_zone();
        usable.loc += self.whole.loc;
        usable
    }

    /// Returns the surface of `layers` under `point`, topmost first, with
    /// its origin in global coordinates.
    pub(crate) fn surface_under(
        self,
        layers: &[Layer],
        point: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let map = layer_map_for_output(self.output);
        let local = point - self.whole.loc.to_f64();
        layers.iter().find_map(|layer| {
            let surface = map.layer_under(*layer, local)?;
            let geometry = map.layer_geometry(surface)?;
            let (under, offset) =
                surface.surface_under(local - geometry.loc.to_f64(), WindowSurfaceType::ALL)?;
            Some((under, (geometry.loc + offset + self.whole.loc).to_f64()))
        })
    }

    /// Returns a layer surface that wants the keyboard to itself, if any: a
    /// launcher or lock screen in the top or overlay layer.
    pub(crate) fn exclusive_keyboard_surface(self) -> Option<WlSurface> {
        let map = layer_map_for_output(self.output);
        ABOVE_WINDOWS.into_iter().find_map(|layer| {
            map.layers_on(layer)
                .find(|surface| {
                    surface.cached_state().keyboard_interactivity
                        == KeyboardInteractivity::Exclusive
                })
                .map(|surface| surface.wl_surface().clone())
        })
    }

    /// Rearranges the layer surfaces after `surface` committed, and sends a
    /// layer surface its first configure. Returns whether `surface` is a
    /// layer surface.
    pub(crate) fn commit(self, surface: &WlSurface) -> bool {
        let mut map = layer_map_for_output(self.output);
        let Some(layer) = map
            .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .cloned()
        else {
            return false;
        };
        map.arrange();
        let is_configured = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .is_some_and(|data| {
                    data.lock()
                        .expect("no thread panics while holding layer surface data")
                        .initial_configure_sent
                })
        });
        if !is_configured {
            layer.layer_surface().send_configure();
        }
        true
    }

    /// Calls `visit` with each layer surface in `layers`, topmost first,
    /// and its origin in global coordinates.
    pub(crate) fn each(
        self,
        layers: &[Layer],
        mut visit: impl FnMut(&LayerSurface, Point<i32, Logical>),
    ) {
        let map: MutexGuard<'_, LayerMap> = layer_map_for_output(self.output);
        for layer in layers {
            // The map lists each layer's surfaces bottom first.
            let surfaces: Vec<&LayerSurface> = map.layers_on(*layer).collect();
            for surface in surfaces.into_iter().rev() {
                if let Some(geometry) = map.layer_geometry(surface) {
                    visit(surface, geometry.loc + self.whole.loc);
                }
            }
        }
    }
}

/// The layer surfaces of every output.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LayerMaps<'outputs> {
    /// The outputs; each one's data holds its layer map.
    outputs: &'outputs Outputs,
}

impl<'outputs> LayerMaps<'outputs> {
    /// The layer surfaces of every one of `outputs`.
    pub(crate) const fn new(outputs: &'outputs Outputs) -> Self {
        Self { outputs }
    }

    /// Returns the outputs.
    pub(crate) const fn outputs(self) -> &'outputs Outputs {
        self.outputs
    }

    /// Returns each output's layer surfaces.
    fn each_output(self) -> impl Iterator<Item = OutputLayers<'outputs>> {
        self.outputs
            .iter()
            .map(|output| OutputLayers::new(output, Outputs::geometry(output)))
    }

    /// Returns the surface of `layers` under `point`, topmost first, with
    /// its origin in global coordinates; a layer surface lies within its
    /// output, so only the output under `point` is looked at.
    pub(crate) fn surface_under(
        self,
        layers: &[Layer],
        point: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.outputs.at(point)?;
        OutputLayers::new(output, Outputs::geometry(output)).surface_under(layers, point)
    }

    /// Returns a layer surface that wants the keyboard to itself on any
    /// output, if one does.
    pub(crate) fn exclusive_keyboard_surface(self) -> Option<WlSurface> {
        self.each_output()
            .find_map(OutputLayers::exclusive_keyboard_surface)
    }

    /// Rearranges the layer surfaces of the output `surface` is on after it
    /// committed; returns whether it is a layer surface.
    pub(crate) fn commit(self, surface: &WlSurface) -> bool {
        self.each_output().any(|output| output.commit(surface))
    }

    /// Calls `visit` with each layer surface in `layers` on every output,
    /// topmost first within each, and its origin in global coordinates.
    pub(crate) fn each(
        self,
        layers: &[Layer],
        mut visit: impl FnMut(&LayerSurface, Point<i32, Logical>),
    ) {
        self.each_by_output(|_| layers, &mut visit);
    }

    /// Calls `visit` with each layer surface on every output, in the layers
    /// `layers_for` chooses for the output given the area it covers,
    /// topmost first within each, and its origin in global coordinates.
    pub(crate) fn each_by_output<'layers>(
        self,
        layers_for: impl Fn(Rectangle<i32, Logical>) -> &'layers [Layer],
        mut visit: impl FnMut(&LayerSurface, Point<i32, Logical>),
    ) {
        for output in self.each_output() {
            output.each(layers_for(output.whole), &mut visit);
        }
    }

    /// Returns the area of the output under `point`, if there is one.
    pub(crate) fn area_at(self, point: Point<f64, Logical>) -> Option<Rectangle<i32, Logical>> {
        self.outputs.at(point).map(Outputs::geometry)
    }
}

//! Layer surfaces (`zwlr_layer_shell_v1`): wallpapers, panels, notifications
//! and launchers that sit in a layer below or above the windows.
//!
//! Smithay's [`LayerMap`] (one per output, kept in the output's data)
//! arranges them by their anchors and exclusive zones; what they leave free
//! is the output's usable area, where windows are maximized and tiled.
//! [`Layers`] keeps each mapped layer surface counted and answers what is
//! where; the scene draws the background and bottom layers below the
//! windows, and the top and overlay layers above them.

use std::sync::MutexGuard;

use smithay::desktop::{LayerMap, LayerSurface, WindowSurfaceType, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer, LayerSurfaceData};

use crate::resources::{ResourceKind, ResourceLedger, Tracked};

/// The layers drawn above the windows, topmost first.
pub(crate) const ABOVE_WINDOWS: [Layer; 2] = [Layer::Overlay, Layer::Top];

/// The layers drawn below the windows, topmost first.
pub(crate) const BELOW_WINDOWS: [Layer; 2] = [Layer::Bottom, Layer::Background];

/// The mapped layer surfaces, counted while they live.
#[derive(Debug, Default)]
pub(crate) struct Layers {
    /// Each mapped layer surface, with its count.
    mapped: Vec<(LayerSurface, Tracked)>,
    /// The layer surface holding the keyboard, if one does.
    keyboard: Option<WlSurface>,
}

impl Layers {
    /// Returns the layer surface holding the keyboard, if one does.
    pub(crate) const fn keyboard(&self) -> Option<&WlSurface> {
        self.keyboard.as_ref()
    }

    /// Returns whether the layer surface `surface` takes the keyboard when
    /// clicked.
    pub(crate) fn takes_keyboard_on_click(&self, surface: &WlSurface) -> bool {
        self.find(surface).is_some_and(|layer| {
            layer.cached_state().keyboard_interactivity != KeyboardInteractivity::None
        })
    }

    /// Records that `surface`, a layer surface, holds the keyboard, or with
    /// `None` that none does; returns whether that changed.
    pub(crate) fn set_keyboard(&mut self, surface: Option<WlSurface>) -> bool {
        let changes = self.keyboard != surface;
        self.keyboard = surface;
        changes
    }

    /// Returns the layer surface whose root is `surface`, if it is one.
    pub(crate) fn find(&self, surface: &WlSurface) -> Option<&LayerSurface> {
        self.mapped
            .iter()
            .map(|(layer, _)| layer)
            .find(|layer| layer.wl_surface() == surface)
    }

    /// Returns every mapped layer surface.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &LayerSurface> {
        self.mapped.iter().map(|(layer, _)| layer)
    }

    /// Maps `surface` on `output`; it is configured when it first commits.
    pub(crate) fn map(&mut self, surface: LayerSurface, output: &Output, ledger: &ResourceLedger) {
        let mut map = layer_map_for_output(output);
        if let Err(error) = map.map_layer(&surface) {
            tracing::warn!(%error, "could not map a layer surface");
            return;
        }
        tracing::debug!(namespace = surface.namespace(), "layer surface mapped");
        self.mapped
            .push((surface, ledger.track(ResourceKind::LayerSurface)));
    }

    /// Unmaps the layer surface whose root is `surface`, which is gone.
    pub(crate) fn unmap(&mut self, surface: &WlSurface, output: &Output) {
        let mut map = layer_map_for_output(output);
        self.mapped.retain(|(layer, _)| {
            let is_gone = layer.wl_surface() == surface;
            if is_gone {
                map.unmap_layer(layer);
            }
            !is_gone
        });
        map.arrange();
        if self.keyboard.as_ref() == Some(surface) {
            self.keyboard = None;
        }
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
        layers: [Layer; 2],
        point: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let map = layer_map_for_output(self.output);
        let local = point - self.whole.loc.to_f64();
        layers.into_iter().find_map(|layer| {
            let surface = map.layer_under(layer, local)?;
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
        layers: [Layer; 2],
        mut visit: impl FnMut(&LayerSurface, Point<i32, Logical>),
    ) {
        let map: MutexGuard<'_, LayerMap> = layer_map_for_output(self.output);
        for layer in layers {
            // The map lists each layer's surfaces bottom first.
            let surfaces: Vec<&LayerSurface> = map.layers_on(layer).collect();
            for surface in surfaces.into_iter().rev() {
                if let Some(geometry) = map.layer_geometry(surface) {
                    visit(surface, geometry.loc + self.whole.loc);
                }
            }
        }
    }
}

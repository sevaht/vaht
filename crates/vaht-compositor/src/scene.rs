//! What an output shows: the elements of one frame, topmost first.
//!
//! Composition is the single walk over what is visible. Drawing uses its
//! elements; hit-testing walks the same stacking order (01 §9). Each window
//! contributes its client surfaces, then the frame vaht draws beneath them;
//! the taskbar, menus and the wallpaper will join as further [`SceneElement`]
//! variants.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::utils::Scale;
use vaht_ui::theme::Metrics;

use crate::windows::Windows;

/// What is drawn where nothing else is: a dark grey, until wallpapers exist.
pub(crate) const BACKGROUND_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];

smithay::backend::renderer::element::render_elements! {
    /// One drawable part of a frame.
    pub(crate) SceneElement<R> where R: ImportAll + ImportMem;
    /// A client surface: a window, one of its subsurfaces, or a popup.
    Surface = WaylandSurfaceRenderElement<R>,
    /// A panel vaht drew itself, such as a titlebar.
    Panel = MemoryRenderBufferRenderElement<R>,
    /// A plain colored rectangle, such as a border line.
    Solid = SolidColorRenderElement,
}

/// What an output shows: the windows, and how their frames are sized.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scene<'windows> {
    /// The windows, in stacking order.
    windows: &'windows Windows,
    /// The theme's sizes, which frames are drawn with.
    metrics: Metrics,
}

impl<'windows> Scene<'windows> {
    /// Creates the scene of `windows`, their frames sized by `metrics`.
    pub(crate) const fn new(windows: &'windows Windows, metrics: Metrics) -> Self {
        Self { windows, metrics }
    }

    /// Returns the elements to draw at `scale`, topmost first, as damage
    /// tracking expects.
    pub(crate) fn elements<R>(&self, renderer: &mut R, scale: Scale<f64>) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let mut elements = Vec::new();
        for window in self.windows.top_down() {
            let origin = window.surface_origin().to_physical_precise_round(scale);
            elements.extend(
                window
                    .toplevel()
                    .render_elements(renderer, origin, scale)
                    .into_iter()
                    .map(SceneElement::Surface),
            );
            let geometry = window.frame_geometry(self.metrics);
            elements.extend(window.frame().render_elements(renderer, &geometry, scale));
        }
        elements
    }
}

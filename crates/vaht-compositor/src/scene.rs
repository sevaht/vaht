//! What an output shows: the elements of one frame, topmost first.
//!
//! Composition is the single walk over what is visible. Drawing uses its
//! elements; hit-testing walks the same stacking order (01 §9). Today a frame
//! holds client windows; frames, the taskbar, menus and the wallpaper join as
//! further [`SceneElement`] variants.

use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::utils::Scale;

use crate::windows::Windows;

smithay::backend::renderer::element::render_elements! {
    /// One drawable part of a frame.
    pub(crate) SceneElement<R> where R: ImportAll + ImportMem;
    /// A client surface: a window, one of its subsurfaces, or a popup.
    Surface = WaylandSurfaceRenderElement<R>,
}

/// Returns the elements of a frame of `windows`, topmost first, as damage
/// tracking expects.
pub(crate) fn compose<R>(
    renderer: &mut R,
    windows: &Windows,
    scale: Scale<f64>,
) -> Vec<SceneElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + 'static,
{
    windows
        .top_down()
        .flat_map(|window| {
            let origin = window.surface_origin().to_physical_precise_round(scale);
            window.toplevel().render_elements(renderer, origin, scale)
        })
        .map(SceneElement::Surface)
        .collect()
}

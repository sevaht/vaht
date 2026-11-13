//! What an output shows: the elements of one frame, topmost first.
//!
//! Composition is the single walk over what is visible: drawing uses its
//! elements, and hit-testing ([`Scene::target_at`]) walks the same windows
//! in the same order, so what is clicked is always what is seen (01 §9). Each window
//! contributes its client surfaces, then the frame vaht draws beneath them;
//! the taskbar, menus and the wallpaper will join as further [`SceneElement`]
//! variants.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::utils::{Logical, Point, Scale};
use vaht_ui::theme::Metrics;

use crate::window::Window;
use crate::windows::{WindowTarget, Windows};
use crate::workspaces::Workspace;

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

/// What an output shows: the windows of a workspace, and how their frames
/// are sized.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scene<'windows> {
    /// The windows, in stacking order.
    windows: &'windows Windows,
    /// The theme's sizes, which frames are drawn with.
    metrics: Metrics,
    /// The workspace shown.
    workspace: Workspace,
}

impl<'windows> Scene<'windows> {
    /// Creates the scene of `windows` shown while `workspace` is, their
    /// frames sized by `metrics`.
    pub(crate) const fn new(
        windows: &'windows Windows,
        metrics: Metrics,
        workspace: Workspace,
    ) -> Self {
        Self {
            windows,
            metrics,
            workspace,
        }
    }

    /// Returns the windows shown, from top to bottom.
    pub(crate) fn windows(&self) -> impl Iterator<Item = &'windows Window> + 'windows {
        self.windows.shown_top_down(self.workspace)
    }

    /// Returns what is under `point` (global coordinates): the topmost
    /// shown window's client surface or frame there.
    pub(crate) fn target_at(&self, point: Point<f64, Logical>) -> Option<WindowTarget> {
        self.windows().find_map(|window| {
            let id = window.id();
            if window.shows_content()
                && let Some((surface, origin)) = window.surface_under(point)
            {
                return Some(WindowTarget::Surface {
                    window: id,
                    surface,
                    origin,
                });
            }
            if !window.has_frame() {
                return None;
            }
            let part = window.frame_geometry(self.metrics).part_at(point)?;
            Some(WindowTarget::Frame { window: id, part })
        })
    }

    /// Returns the elements to draw at `scale`, topmost first, as damage
    /// tracking expects.
    pub(crate) fn elements<R>(&self, renderer: &mut R, scale: Scale<f64>) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let mut elements = Vec::new();
        for window in self.windows() {
            if window.shows_content() {
                let origin = window.surface_origin().to_physical_precise_round(scale);
                elements.extend(
                    window
                        .toplevel()
                        .render_elements(renderer, origin, scale)
                        .into_iter()
                        .map(SceneElement::Surface),
                );
            }
            if window.has_frame() {
                let geometry = window.frame_geometry(self.metrics);
                elements.extend(window.frame().render_elements(renderer, &geometry, scale));
            }
        }
        elements
    }
}

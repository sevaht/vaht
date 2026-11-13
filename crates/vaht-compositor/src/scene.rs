//! What an output shows: the elements of one frame, topmost first.
//!
//! Composition is the single walk over what is visible: drawing uses its
//! elements, and hit-testing ([`Scene::target_at`]) walks the same layers
//! and windows in the same order, so what is clicked is always what is seen
//! (01 §9). From the top: the overlay and top layers, the windows, then the
//! bottom and background layers. Each window
//! contributes its client surfaces, then the frame vaht draws beneath them;
//! the taskbar, menus and the wallpaper will join as further [`SceneElement`]
//! variants.

use smithay::backend::renderer::element::AsRenderElements as _;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Scale};
use smithay::wayland::shell::wlr_layer::Layer;
use vaht_ui::theme::Metrics;

use crate::layers::{ABOVE_WINDOWS, BELOW_WINDOWS, OutputLayers};
use crate::window::{Window, WindowId};
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

/// What is under the pointer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PointerTarget {
    /// A window, or its frame.
    Window(WindowTarget),
    /// A layer surface.
    Layer {
        /// The surface under the pointer (the layer surface, or one of its
        /// subsurfaces).
        surface: WlSurface,
        /// That surface's origin, in global coordinates.
        origin: Point<f64, Logical>,
        /// Whether its layer is below the windows: a wallpaper or a desktop
        /// widget, which counts as the desktop for bindings.
        is_below_windows: bool,
    },
}

impl PointerTarget {
    /// Returns the window under the pointer, if it is a window.
    pub(crate) const fn window(&self) -> Option<WindowId> {
        match self {
            Self::Window(target) => Some(target.window()),
            Self::Layer { .. } => None,
        }
    }

    /// Returns the client surface under the pointer and its origin, if the
    /// pointer is over one rather than over a frame.
    pub(crate) fn client_surface(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        match self {
            Self::Window(WindowTarget::Surface {
                surface, origin, ..
            })
            | Self::Layer {
                surface, origin, ..
            } => Some((surface.clone(), *origin)),
            Self::Window(WindowTarget::Frame { .. }) => None,
        }
    }
}

/// What an output shows: its layer surfaces, and the windows of a workspace
/// with frames sized by the theme.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scene<'windows> {
    /// The windows, in stacking order.
    windows: &'windows Windows,
    /// The theme's sizes, which frames are drawn with.
    metrics: Metrics,
    /// The workspace shown.
    workspace: Workspace,
    /// The output's layer surfaces.
    layers: OutputLayers<'windows>,
}

impl<'windows> Scene<'windows> {
    /// Creates the scene of `windows` shown while `workspace` is, their
    /// frames sized by `metrics`, between the output's `layers`.
    pub(crate) const fn new(
        windows: &'windows Windows,
        metrics: Metrics,
        workspace: Workspace,
        layers: OutputLayers<'windows>,
    ) -> Self {
        Self {
            windows,
            metrics,
            workspace,
            layers,
        }
    }

    /// Returns the windows shown, from top to bottom.
    pub(crate) fn windows(&self) -> impl Iterator<Item = &'windows Window> + 'windows {
        self.windows.shown_top_down(self.workspace)
    }

    /// Returns what is under `point` (global coordinates): a layer surface
    /// above the windows, the topmost shown window's client surface or
    /// frame, or a layer surface below the windows.
    pub(crate) fn target_at(&self, point: Point<f64, Logical>) -> Option<PointerTarget> {
        let layer = |layers, is_below_windows| {
            self.layers
                .surface_under(layers, point)
                .map(|(surface, origin)| PointerTarget::Layer {
                    surface,
                    origin,
                    is_below_windows,
                })
        };
        layer(ABOVE_WINDOWS, false)
            .or_else(|| self.window_at(point).map(PointerTarget::Window))
            .or_else(|| layer(BELOW_WINDOWS, true))
    }

    /// Returns the elements to draw at `scale`, topmost first, as damage
    /// tracking expects.
    pub(crate) fn elements<R>(&self, renderer: &mut R, scale: Scale<f64>) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let mut elements = Vec::new();
        self.push_layers(&mut elements, renderer, ABOVE_WINDOWS, scale);
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
        self.push_layers(&mut elements, renderer, BELOW_WINDOWS, scale);
        elements
    }

    /// Returns the topmost shown window's client surface or frame at
    /// `point`.
    fn window_at(&self, point: Point<f64, Logical>) -> Option<WindowTarget> {
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

    /// Appends the elements of the layer surfaces in `layers`.
    fn push_layers<R>(
        &self,
        elements: &mut Vec<SceneElement<R>>,
        renderer: &mut R,
        layers: [Layer; 2],
        scale: Scale<f64>,
    ) where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        self.layers.each(layers, |surface, origin| {
            let surface_elements: Vec<WaylandSurfaceRenderElement<R>> = surface.render_elements(
                renderer,
                origin.to_physical_precise_round(scale),
                scale,
                1.0,
            );
            elements.extend(surface_elements.into_iter().map(SceneElement::Surface));
        });
    }
}

//! What an output shows: the elements of one frame, topmost first.
//!
//! Composition is the single walk over what is visible: drawing uses its
//! elements, and hit-testing ([`Scene::target_at`]) walks the same layers
//! and windows in the same order, so what is clicked is always what is seen
//! (01 §9). From the top: the overlay and top layers, the taskbar, the
//! windows, then the bottom and background layers. Each window contributes
//! its client surfaces, then the frame vaht draws beneath them; menus and
//! the wallpaper will join as further [`SceneElement`] variants.
//!
//! The scene joins four sources: the window manager says which windows are
//! shown and where, the [`Toplevels`] draw and hit-test their surfaces,
//! [`FrameTextures`] draws their frames, and the [`Taskbar`] itself.

mod frame_textures;
mod panel_texture;

use smithay::backend::renderer::element::AsRenderElements as _;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Scale};
use smithay::wayland::shell::wlr_layer::Layer;

pub(crate) use self::frame_textures::FrameTextures;
pub(crate) use self::panel_texture::PanelTexture;
use crate::layers::{ABOVE_FULLSCREEN, ABOVE_WINDOWS, BELOW_WINDOWS, OutputLayers};
use crate::taskbar::{Taskbar, TaskbarHit};
use crate::toplevels::{Toplevel, Toplevels};
use crate::window_manager::{FramePart, Window, WindowId, WindowManager};

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

/// What of a window is under a point.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WindowTarget {
    /// One of the client's surfaces.
    Surface {
        /// The window it belongs to.
        window: WindowId,
        /// The surface.
        surface: WlSurface,
        /// The surface's origin, in global coordinates.
        origin: Point<f64, Logical>,
    },
    /// Part of the frame vaht draws around it.
    Frame {
        /// The window the frame surrounds.
        window: WindowId,
        /// Which part.
        part: FramePart,
    },
}

impl WindowTarget {
    /// Returns the window the target belongs to.
    pub(crate) const fn window(&self) -> WindowId {
        match self {
            Self::Surface { window, .. } | Self::Frame { window, .. } => *window,
        }
    }
}

/// What is under the pointer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PointerTarget {
    /// A window, or its frame.
    Window(WindowTarget),
    /// The taskbar: one of its widgets, or its background.
    Taskbar(Option<TaskbarHit>),
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
    /// Returns the window under the pointer, if it is a window: the one a
    /// press focuses, and sloppy focus follows.
    pub(crate) const fn window(&self) -> Option<WindowId> {
        match self {
            Self::Window(target) => Some(target.window()),
            Self::Taskbar(_) | Self::Layer { .. } => None,
        }
    }

    /// Returns the window a binding run here acts on: the window under the
    /// pointer, or the one whose taskbar entry it is on.
    pub(crate) const fn binding_window(&self) -> Option<WindowId> {
        match self {
            Self::Taskbar(Some(hit)) => hit.window(),
            Self::Window(_) | Self::Taskbar(None) | Self::Layer { .. } => self.window(),
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
            Self::Window(WindowTarget::Frame { .. }) | Self::Taskbar(_) => None,
        }
    }
}

/// What an output shows: its layer surfaces, and the windows shown, with
/// their frames.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scene<'state> {
    /// Which windows are shown, and where.
    window_manager: &'state WindowManager,
    /// The windows' surfaces.
    toplevels: &'state Toplevels,
    /// The windows' frames, as the renderer takes them.
    frame_textures: &'state FrameTextures,
    /// The taskbar, if there is one.
    taskbar: Option<&'state Taskbar>,
    /// The output's layer surfaces.
    layers: OutputLayers<'state>,
}

impl<'state> Scene<'state> {
    /// Creates the scene of the windows `window_manager` shows, drawn from
    /// their `toplevels` and `frame_textures`, under the `taskbar`, between
    /// the output's `layers`.
    pub(crate) const fn new(
        window_manager: &'state WindowManager,
        toplevels: &'state Toplevels,
        frame_textures: &'state FrameTextures,
        taskbar: Option<&'state Taskbar>,
        layers: OutputLayers<'state>,
    ) -> Self {
        Self {
            window_manager,
            toplevels,
            frame_textures,
            taskbar,
            layers,
        }
    }

    /// Returns the windows shown, from top to bottom, with their toplevels.
    pub(crate) fn windows(
        &self,
    ) -> impl Iterator<Item = (&'state Window, &'state Toplevel)> + 'state {
        let toplevels = self.toplevels;
        self.window_manager
            .shown_top_down()
            .filter_map(move |window| Some((window, toplevels.get(window.id())?)))
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
        layer(self.layers_above_windows(), false)
            .or_else(|| {
                self.shown_taskbar()
                    .filter(|taskbar| taskbar.covers(point))
                    .map(|taskbar| PointerTarget::Taskbar(taskbar.hit_at(point)))
            })
            .or_else(|| self.window_at(point).map(PointerTarget::Window))
            .or_else(|| layer(&BELOW_WINDOWS, true))
    }

    /// Returns the elements to draw at `scale`, topmost first, as damage
    /// tracking expects.
    pub(crate) fn elements<R>(&self, renderer: &mut R, scale: Scale<f64>) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let mut elements = Vec::new();
        self.push_layers(&mut elements, renderer, self.layers_above_windows(), scale);
        elements.extend(
            self.shown_taskbar()
                .and_then(|taskbar| taskbar.render_element(renderer, scale)),
        );
        let metrics = self.window_manager.metrics();
        for (window, toplevel) in self.windows() {
            if window.shows_content() {
                let origin = window.surface_origin().to_physical_precise_round(scale);
                elements.extend(
                    toplevel
                        .render_elements(renderer, origin, scale)
                        .into_iter()
                        .map(SceneElement::Surface),
                );
            }
            if window.has_frame() {
                let geometry = window.frame_geometry(metrics);
                elements.extend(self.frame_textures.render_elements(
                    window.id(),
                    &geometry,
                    renderer,
                    scale,
                ));
            }
        }
        self.push_layers(&mut elements, renderer, &BELOW_WINDOWS, scale);
        elements
    }

    /// Returns whether the topmost window shown is fullscreen: it then
    /// covers the top layer and the taskbar, which neither show nor take
    /// clicks, and only the overlay layer is above it (01 §9).
    fn is_fullscreen_on_top(&self) -> bool {
        self.window_manager
            .shown_top_down()
            .next()
            .is_some_and(|window| window.placement().is_fullscreen())
    }

    /// Returns the layers above the windows: the overlay and top layers, or
    /// only the overlay above a fullscreen window.
    fn layers_above_windows(&self) -> &'static [Layer] {
        if self.is_fullscreen_on_top() {
            &ABOVE_FULLSCREEN
        } else {
            &ABOVE_WINDOWS
        }
    }

    /// Returns the taskbar, unless a fullscreen window covers it.
    fn shown_taskbar(&self) -> Option<&'state Taskbar> {
        self.taskbar.filter(|_| !self.is_fullscreen_on_top())
    }

    /// Returns the topmost shown window's client surface or frame at
    /// `point`.
    fn window_at(&self, point: Point<f64, Logical>) -> Option<WindowTarget> {
        let metrics = self.window_manager.metrics();
        self.windows().find_map(|(window, toplevel)| {
            let id = window.id();
            let surface_origin = window.surface_origin();
            if window.shows_content()
                && let Some((surface, offset)) =
                    toplevel.surface_under(point - surface_origin.to_f64())
            {
                let origin = (offset + surface_origin).to_f64();
                return Some(WindowTarget::Surface {
                    window: id,
                    surface,
                    origin,
                });
            }
            if !window.has_frame() {
                return None;
            }
            let part = window.frame_geometry(metrics).part_at(point)?;
            Some(WindowTarget::Frame { window: id, part })
        })
    }

    /// Appends the elements of the layer surfaces in `layers`.
    fn push_layers<R>(
        &self,
        elements: &mut Vec<SceneElement<R>>,
        renderer: &mut R,
        layers: &[Layer],
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

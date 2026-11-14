//! What an output shows: the elements of one frame, topmost first.
//!
//! Composition is the single walk over what is visible: drawing uses its
//! elements, and hit-testing ([`Scene::target_at`]) walks the same layers
//! and windows in the same order, so what is clicked is always what is seen
//! (01 §9). From the top: the overlay and top layers, the windows in the
//! above-dock stacking layer, the taskbar, the other windows, then the
//! bottom and background layers, then the wallpaper. Each
//! window contributes its client surfaces, then, if it is pseudo-transparent,
//! the piece of wallpaper beneath them, then the frame vaht draws.
//!
//! The scene joins four sources: the window manager says which windows are
//! shown and where, the [`Toplevels`] draw and hit-test their surfaces,
//! [`FrameTextures`] draws their frames, and the [`Taskbar`] itself.

mod backdrop;
mod corner_texture;
mod frame_textures;
mod menu_texture;
mod outline;
mod panel_texture;
mod shaped_element;
mod snap_overlay;

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::element::{AsRenderElements as _, Element as _};
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::desktop::WindowSurfaceType;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Scale};
use smithay::wayland::shell::wlr_layer::Layer;

pub(crate) use self::backdrop::Backdrop;
pub(crate) use self::corner_texture::CornerTexture;
pub(crate) use self::frame_textures::FrameTextures;
pub(crate) use self::menu_texture::MenuTexture;
pub(crate) use self::outline::Outline;
pub(crate) use self::panel_texture::PanelTexture;
pub(crate) use self::shaped_element::ShapedElement;
pub(crate) use self::snap_overlay::SnapOverlay;
use crate::cursor::Cursor;
use crate::layers::{ABOVE_FULLSCREEN, ABOVE_WINDOWS, BELOW_WINDOWS, LayerMaps};
use crate::menu::MenuStack;
use crate::outputs::Outputs;
use crate::taskbar::{Taskbar, TaskbarHit};
use crate::tooltip::Tooltip;
use crate::toplevels::{Toplevel, Toplevels};
use crate::window_manager::{FramePart, Window, WindowId, WindowManager};
use crate::xwayland::{WindowShape, XwaylandServer};

/// What is drawn where nothing else is, the wallpaper included: a dark
/// grey, [`Backdrop`]'s own background.
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
    /// A client surface drawn only inside its window's shape (an X11
    /// window's X Shape).
    Shaped = ShapedElement<WaylandSurfaceRenderElement<R>>,
}

/// One drawable part of a frame, moved from global coordinates to where
/// it is on one output.
pub(crate) type OutputElement<R> = RelocateRenderElement<SceneElement<R>>;

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
    /// An X11 program's override-redirect window: its menu, tooltip or
    /// drop-down.
    Unmanaged {
        /// The surface under the pointer.
        surface: WlSurface,
        /// That surface's origin, in global coordinates.
        origin: Point<f64, Logical>,
    },
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
            Self::Taskbar(_) | Self::Layer { .. } | Self::Unmanaged { .. } => None,
        }
    }

    /// Returns the window a binding run here acts on: the window under the
    /// pointer, or the one whose taskbar entry it is on.
    pub(crate) const fn binding_window(&self) -> Option<WindowId> {
        match self {
            Self::Taskbar(Some(hit)) => hit.window(),
            Self::Window(_) | Self::Taskbar(None) | Self::Layer { .. } | Self::Unmanaged { .. } => {
                self.window()
            }
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
            }
            | Self::Unmanaged { surface, origin } => Some((surface.clone(), *origin)),
            Self::Window(WindowTarget::Frame { .. }) | Self::Taskbar(_) => None,
        }
    }
}

/// What the outputs show: their layer surfaces, and the windows shown,
/// with their frames, in global coordinates; each output draws its part.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scene<'state> {
    /// Which windows are shown, and where.
    window_manager: &'state WindowManager,
    /// The windows' surfaces.
    toplevels: &'state Toplevels,
    /// What vaht draws itself.
    drawn: Drawn<'state>,
    /// The outputs' layer surfaces.
    layers: LayerMaps<'state>,
    /// The pointer and where it is, when vaht draws it (on outputs no
    /// host draws it on, and in screenshots that ask for it).
    pointer: Option<(&'state Cursor, Point<f64, Logical>)>,
}

/// What vaht draws itself: the windows' frames, the taskbar, the menus and
/// a resize's outline over the windows, and the wallpaper beneath them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Drawn<'state> {
    /// The windows' frames, as the renderer takes them.
    pub(crate) frames: &'state FrameTextures,
    /// The taskbar, if there is one.
    pub(crate) taskbar: Option<&'state Taskbar>,
    /// The menus open.
    pub(crate) menus: &'state MenuStack,
    /// The wallpaper.
    pub(crate) backdrop: &'state Backdrop,
    /// The outline of an outline resize.
    pub(crate) outline: &'state Outline,
    /// The tooltip, if one shows.
    pub(crate) tooltip: &'state Tooltip,
    /// X11 programs' override-redirect windows, when Xwayland runs.
    pub(crate) xwayland: Option<&'state XwaylandServer>,
    /// Where a moved window would snap, and the layout picker.
    pub(crate) snap: &'state SnapOverlay,
}

impl<'state> Scene<'state> {
    /// Creates the scene of the windows `window_manager` shows, drawn from
    /// their `toplevels`, with what vaht has `drawn` itself, between the
    /// outputs' `layers`.
    pub(crate) const fn new(
        window_manager: &'state WindowManager,
        toplevels: &'state Toplevels,
        drawn: Drawn<'state>,
        layers: LayerMaps<'state>,
    ) -> Self {
        Self {
            window_manager,
            toplevels,
            drawn,
            layers,
            pointer: None,
        }
    }

    /// Returns the scene with `cursor` drawn at `location` (global
    /// coordinates), over everything.
    #[must_use]
    pub(crate) const fn with_pointer(
        mut self,
        cursor: &'state Cursor,
        location: Point<f64, Logical>,
    ) -> Self {
        self.pointer = Some((cursor, location));
        self
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
        self.unmanaged_at(point)
            .or_else(|| layer(self.layers_above_windows(), false))
            .or_else(|| self.window_at(point, true).map(PointerTarget::Window))
            .or_else(|| {
                self.shown_taskbar()
                    .filter(|taskbar| taskbar.covers(point))
                    .map(|taskbar| PointerTarget::Taskbar(taskbar.hit_at(point)))
            })
            .or_else(|| self.window_at(point, false).map(PointerTarget::Window))
            .or_else(|| layer(&BELOW_WINDOWS, true))
    }

    /// Returns the elements to draw at `scale`, topmost first, as damage
    /// tracking expects.
    fn elements<R>(&self, renderer: &mut R, scale: Scale<f64>) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        // The pointer is above everything; then the menus, above even the
        // overlay layer.
        let mut elements = self
            .pointer
            .map(|(cursor, location)| cursor.render_elements(renderer, location, scale))
            .unwrap_or_default();
        elements.extend(self.drawn.tooltip.render_elements(renderer, scale));
        elements.extend(self.drawn.menus.render_elements(renderer, scale));
        elements.extend(self.drawn.outline.render_elements(renderer, scale));
        elements.extend(self.drawn.snap.picker_element(renderer, scale));
        // An X program's menus and tooltips are over every window.
        for window in self
            .drawn
            .xwayland
            .into_iter()
            .flat_map(XwaylandServer::unmanaged)
        {
            // Its geometry is its own; where it is on screen is where X
            // last placed it.
            let origin = window.last_configure().loc.to_physical_precise_round(scale);
            let surface_elements: Vec<WaylandSurfaceRenderElement<R>> =
                window.render_elements(renderer, origin, scale, 1.0);
            let shape = self
                .drawn
                .xwayland
                .and_then(|server| server.shape_of(window));
            elements.extend(Self::clipped(surface_elements, shape, origin, scale));
        }
        self.push_layers(&mut elements, renderer, self.layers_above_windows(), scale);
        self.push_windows(&mut elements, renderer, scale, true);
        elements.extend(
            self.shown_taskbar()
                .and_then(|taskbar| taskbar.render_element(renderer, scale)),
        );
        self.push_windows(&mut elements, renderer, scale, false);
        self.push_layers(&mut elements, renderer, &BELOW_WINDOWS, scale);
        elements.extend(self.drawn.backdrop.render_elements(renderer, scale));
        elements
    }

    /// Returns the elements `output` shows, topmost first, at its scale
    /// and relative to its top left corner.
    pub(crate) fn output_elements<R>(
        &self,
        renderer: &mut R,
        output: &Output,
    ) -> Vec<OutputElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let scale = output.current_scale().fractional_scale().into();
        let corner: Point<i32, Physical> = Outputs::geometry(output)
            .loc
            .to_physical_precise_round(scale);
        let toward_origin = Point::from((-corner.x, -corner.y));
        self.elements(renderer, scale)
            .into_iter()
            .map(|element| {
                RelocateRenderElement::from_element(element, toward_origin, Relocate::Relative)
            })
            .collect()
    }

    /// Appends the elements of the windows shown, topmost first: those in
    /// the above-dock layer, over the taskbar, when `is_above_dock`, else
    /// the rest.
    fn push_windows<R>(
        &self,
        elements: &mut Vec<SceneElement<R>>,
        renderer: &mut R,
        scale: Scale<f64>,
        is_above_dock: bool,
    ) where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let metrics = self.window_manager.metrics();
        let windows = self
            .windows()
            .filter(|(window, _)| self.window_manager.is_above_dock(window.id()) == is_above_dock);
        for (window, toplevel) in windows {
            if window.shows_content() {
                let origin = window.surface_origin().to_physical_precise_round(scale);
                let surface_elements = toplevel.render_elements(
                    renderer,
                    origin,
                    scale,
                    self.window_manager.opacity(window),
                );
                let shape = toplevel
                    .x11()
                    .zip(self.drawn.xwayland)
                    .and_then(|(surface, server)| server.shape_of(surface));
                elements.extend(Self::clipped(surface_elements, shape, origin, scale));
                elements.extend(
                    self.drawn
                        .backdrop
                        .piece_element(window.id(), renderer, scale),
                );
            }
            if window.has_frame() {
                let geometry = window.frame_geometry(metrics);
                elements.extend(self.drawn.frames.render_elements(
                    window.id(),
                    &geometry,
                    renderer,
                    scale,
                ));
            }
            // Where a moved window would snap is just under it.
            elements.extend(self.drawn.snap.preview_elements(window.id(), scale));
        }
    }

    /// Returns `surface_elements` as scene elements, clipped to `shape`
    /// (relative to the window, whose corner is at `corner`) if the window
    /// is shaped.
    fn clipped<R>(
        surface_elements: Vec<WaylandSurfaceRenderElement<R>>,
        shape: Option<&WindowShape>,
        corner: Point<i32, Physical>,
        scale: Scale<f64>,
    ) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some(shape) = shape else {
            return surface_elements
                .into_iter()
                .map(SceneElement::Surface)
                .collect();
        };
        let clip = shape.clip(scale);
        surface_elements
            .into_iter()
            .map(|element| {
                let id = shape.element_id(element.id());
                SceneElement::Shaped(ShapedElement::new(element, &clip, corner, (id, scale)))
            })
            .collect()
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

    /// Returns the taskbar, unless it is auto-hidden or a fullscreen window
    /// covers it.
    fn shown_taskbar(&self) -> Option<&'state Taskbar> {
        self.drawn
            .taskbar
            .filter(|taskbar| taskbar.is_shown() && !self.is_fullscreen_on_top())
    }

    /// Returns the topmost shown window's client surface or frame at
    /// `point`, among those in the above-dock layer when `is_above_dock`,
    /// else among the rest.
    fn window_at(&self, point: Point<f64, Logical>, is_above_dock: bool) -> Option<WindowTarget> {
        let metrics = self.window_manager.metrics();
        let windows = self
            .windows()
            .filter(|(window, _)| self.window_manager.is_above_dock(window.id()) == is_above_dock);
        windows.into_iter().find_map(|(window, toplevel)| {
            let id = window.id();
            let surface_origin = window.surface_origin();
            let shape = toplevel
                .x11()
                .zip(self.drawn.xwayland)
                .and_then(|(surface, server)| server.shape_of(surface));
            let is_in_shape =
                shape.is_none_or(|shape| shape.contains(point - surface_origin.to_f64()));
            if window.shows_content()
                && is_in_shape
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

    /// Returns the override-redirect window under `point`, topmost first.
    fn unmanaged_at(&self, point: Point<f64, Logical>) -> Option<PointerTarget> {
        self.drawn
            .xwayland
            .into_iter()
            .flat_map(XwaylandServer::unmanaged)
            .find_map(|window| {
                // The point is relative to the window; the origin comes
                // back in global coordinates.
                let corner = window.last_configure().loc;
                let shape = self
                    .drawn
                    .xwayland
                    .and_then(|server| server.shape_of(window));
                if shape.is_some_and(|shape| !shape.contains(point - corner.to_f64())) {
                    return None;
                }
                let (surface, origin) = window.surface_under(
                    point - corner.to_f64(),
                    corner,
                    WindowSurfaceType::ALL,
                )?;
                Some(PointerTarget::Unmanaged {
                    surface,
                    origin: origin.to_f64(),
                })
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

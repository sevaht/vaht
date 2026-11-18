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
mod blur;
mod color_element;
mod corner_texture;
mod frame_textures;
mod menu_texture;
mod outline;
mod panel_texture;
mod shaped_element;
mod snap_overlay;

use std::collections::HashMap;

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::surface::render_elements_from_surface_tree;
use smithay::backend::renderer::element::utils::select_dmabuf_feedback;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::element::{AsRenderElements as _, Element as _};
use smithay::backend::renderer::element::{
    Id, RenderElementStates, default_primary_scanout_output_compare,
};
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::desktop::utils::{
    OutputPresentationFeedback, send_dmabuf_feedback_surface_tree,
    surface_presentation_feedback_flags_from_states, surface_primary_scanout_output,
    take_presentation_feedback_surface_tree, under_from_surface_tree,
    update_surface_primary_scanout_output, with_surfaces_surface_tree,
};
use smithay::desktop::{PopupManager, WindowSurfaceType, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Scale};
use smithay::wayland::compositor::{TraversalAction, with_surface_tree_downward};
use smithay::wayland::dmabuf::DmabufFeedback;
use smithay::wayland::shell::wlr_layer::Layer;
use smithay::xwayland::X11Surface;

pub(crate) use self::backdrop::Backdrop;
pub(crate) use self::blur::{BlurElement, BlurRenderer};
pub(crate) use self::color_element::{ColorElement, ColorRenderer};
pub(crate) use self::corner_texture::CornerTexture;
pub(crate) use self::frame_textures::FrameTextures;
pub(crate) use self::menu_texture::MenuTexture;
pub(crate) use self::outline::Outline;
pub(crate) use self::panel_texture::PanelTexture;
pub(crate) use self::shaped_element::ShapedElement;
pub(crate) use self::snap_overlay::SnapOverlay;
use crate::color::{Conversion, ImageDescription};
use crate::config::BlurSettings;
use crate::cursor::Cursor;
use crate::layers::{ABOVE_FULLSCREEN, ABOVE_WINDOWS, ALL_LAYERS, BELOW_WINDOWS, LayerMaps};
use crate::menu::MenuStack;
use crate::outputs::Outputs;
use crate::protocols::color_management;
use crate::session_lock::SessionLock;
use crate::taskbar::{Taskbar, TaskbarHit};
use crate::taskbars::Taskbars;
use crate::tooltip::Tooltip;
use crate::toplevels::{Toplevel, Toplevels};
use crate::window_manager::{FramePart, Window, WindowId, WindowManager};
use crate::xwayland::{WindowShape, XwaylandServer};

/// What is drawn where nothing else is, the wallpaper included: a dark
/// grey, [`Backdrop`]'s own background.
pub(crate) const BACKGROUND_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];

smithay::backend::renderer::element::render_elements! {
    /// One drawable part of a frame.
    pub(crate) SceneElement<R> where R: ImportAll + ImportMem + BlurRenderer;
    /// A client surface: a window, one of its subsurfaces, or a popup.
    Surface = WaylandSurfaceRenderElement<R>,
    /// A panel vaht drew itself, such as a titlebar.
    Panel = MemoryRenderBufferRenderElement<R>,
    /// A plain colored rectangle, such as a border line.
    Solid = SolidColorRenderElement,
    /// A client surface drawn only inside its window's shape (an X11
    /// window's X Shape).
    Shaped = ShapedElement<WaylandSurfaceRenderElement<R>>,
    /// What is behind part of a surface, blurred, as its program asks.
    Blur = BlurElement,
}

/// A renderer that draws scenes: it imports client and vaht's own
/// buffers, and blurs.
pub(crate) trait SceneRenderer:
    Renderer + ImportAll + ImportMem + BlurRenderer + ColorRenderer
{
}

impl<R: Renderer + ImportAll + ImportMem + BlurRenderer + ColorRenderer> SceneRenderer for R {}

/// One drawable part of a frame, moved from global coordinates to where
/// it is on one output, in that output's colours.
pub(crate) type OutputElement<R> = ColorElement<RelocateRenderElement<SceneElement<R>>>;

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
    /// A locker's surface, while the session is locked.
    Lock {
        /// The surface under the pointer.
        surface: WlSurface,
        /// That surface's origin, in global coordinates.
        origin: Point<f64, Logical>,
    },
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
            Self::Taskbar(_) | Self::Layer { .. } | Self::Unmanaged { .. } | Self::Lock { .. } => {
                None
            }
        }
    }

    /// Returns the window a binding run here acts on: the window under the
    /// pointer, or the one whose taskbar entry it is on.
    pub(crate) const fn binding_window(&self) -> Option<WindowId> {
        match self {
            Self::Taskbar(Some(hit)) => hit.window(),
            Self::Window(_)
            | Self::Taskbar(None)
            | Self::Layer { .. }
            | Self::Unmanaged { .. }
            | Self::Lock { .. } => self.window(),
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
            | Self::Unmanaged { surface, origin }
            | Self::Lock { surface, origin } => Some((surface.clone(), *origin)),
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
    /// A window the pointer passes through: one dragged along with a drag
    /// and drop, which is no drop target itself.
    passed_through: Option<WindowId>,
}

/// The `linux-dmabuf` feedback for a client surface on one output.
#[derive(Debug, Clone)]
pub(crate) struct SurfaceDmabufFeedback {
    /// For a surface vaht composites: formats the renderer reads.
    pub(crate) render: DmabufFeedback,
    /// For a surface put straight on a display plane: formats the plane
    /// takes, first.
    pub(crate) scanout: DmabufFeedback,
}

/// What vaht draws itself: the windows' frames, the taskbar, the menus and
/// a resize's outline over the windows, and the wallpaper beneath them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Drawn<'state> {
    /// The windows' frames, as the renderer takes them.
    pub(crate) frames: &'state FrameTextures,
    /// The taskbars.
    pub(crate) taskbars: &'state Taskbars,
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
    /// The session lock: while locked, the outputs show only it.
    pub(crate) lock: &'state SessionLock,
    /// How what is behind surfaces asking for it is blurred.
    pub(crate) blur: BlurSettings,
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
            passed_through: None,
        }
    }

    /// Returns the scene with the pointer passing through `window`, if
    /// given: it is found under no point.
    #[must_use]
    pub(crate) const fn passing_through(mut self, window: Option<WindowId>) -> Self {
        self.passed_through = window;
        self
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
        if self.drawn.lock.is_locked() {
            return self.lock_target_at(point);
        }
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
                self.shown_taskbars()
                    .find(|taskbar| taskbar.covers(point))
                    .map(|taskbar| PointerTarget::Taskbar(taskbar.hit_at(point)))
            })
            .or_else(|| self.window_at(point, false).map(PointerTarget::Window))
            .or_else(|| layer(&BELOW_WINDOWS, true))
    }

    /// Takes the presentation feedback of the client surfaces `output`
    /// shows best, after a frame was drawn on it with the elements
    /// `states`; the backend sends it when the frame shows. A surface on
    /// two outputs is told of the one showing most of it.
    pub(crate) fn presentation_feedback(
        &self,
        output: &Output,
        states: &RenderElementStates,
    ) -> OutputPresentationFeedback {
        let roots = self.client_surfaces(output);
        for root in &roots {
            with_surfaces_surface_tree(root, |surface, data| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    data,
                    None,
                    states,
                    default_primary_scanout_output_compare,
                );
            });
        }
        let mut feedback = OutputPresentationFeedback::new(output);
        for root in &roots {
            take_presentation_feedback_surface_tree(
                root,
                &mut feedback,
                surface_primary_scanout_output,
                |surface, _| surface_presentation_feedback_flags_from_states(surface, None, states),
            );
        }
        feedback
    }

    /// Tells each client surface shown mostly on `output` which buffers
    /// suit it now (`linux-dmabuf` feedback): one the frame just put
    /// straight on a display plane (`states`) is told the formats that
    /// plane takes, so a fullscreen game keeps drawing buffers that can be
    /// scanned out without a copy; the rest, those vaht composites.
    pub(crate) fn send_dmabuf_feedback(
        &self,
        output: &Output,
        feedback: &SurfaceDmabufFeedback,
        states: &RenderElementStates,
    ) {
        for root in self.client_surfaces(output) {
            send_dmabuf_feedback_surface_tree(
                &root,
                output,
                surface_primary_scanout_output,
                |surface, _| {
                    select_dmabuf_feedback(surface, states, &feedback.render, &feedback.scanout)
                },
            );
        }
    }

    /// Returns the root surfaces of the clients `output` may show, each
    /// with its subsurfaces beneath it: the windows and X programs' menus,
    /// the output's layer surfaces, their popups, and while locked only
    /// the output's lock surface.
    fn client_surfaces(&self, output: &Output) -> Vec<WlSurface> {
        if self.drawn.lock.is_locked() {
            return self
                .drawn
                .lock
                .surface_on(output)
                .cloned()
                .into_iter()
                .collect();
        }
        let mut roots: Vec<WlSurface> = self
            .windows()
            .filter_map(|(_, toplevel)| toplevel.wl_surface())
            .collect();
        roots.extend(
            self.drawn
                .xwayland
                .into_iter()
                .flat_map(XwaylandServer::unmanaged)
                .filter_map(X11Surface::wl_surface),
        );
        roots.extend(
            layer_map_for_output(output)
                .layers()
                .map(|layer| layer.wl_surface().clone()),
        );
        let popups: Vec<WlSurface> = roots
            .iter()
            .flat_map(PopupManager::popups_for_surface)
            .map(|(popup, _)| popup.wl_surface().clone())
            .collect();
        roots.extend(popups);
        roots
    }

    /// Returns the elements to draw at `scale`, topmost first, as damage
    /// tracking expects.
    fn elements<R>(&self, renderer: &mut R, scale: Scale<f64>) -> Vec<SceneElement<R>>
    where
        R: SceneRenderer,
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
        elements.extend(self.drawn.outline.render_elements(renderer, scale, 1.0));
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
            self.shown_taskbars()
                .filter_map(|taskbar| taskbar.render_element(renderer, scale)),
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
        target: ImageDescription,
    ) -> Vec<OutputElement<R>>
    where
        R: SceneRenderer,
        R::TextureId: Send + Clone + 'static,
    {
        let scale = output.current_scale().fractional_scale().into();
        let corner: Point<i32, Physical> = Outputs::geometry(output)
            .loc
            .to_physical_precise_round(scale);
        let toward_origin = Point::from((-corner.x, -corner.y));
        let elements = if self.drawn.lock.is_locked() {
            self.lock_elements(renderer, output, corner, scale)
        } else {
            self.elements(renderer, scale)
        };
        self.in_colors_of(elements, target, (toward_origin, scale))
    }

    /// Returns `elements` moved by `offset` and drawn in `target`'s
    /// colours at `scale`: each surface's from what it said its pixels
    /// mean, everything else from sRGB.
    fn in_colors_of<R>(
        &self,
        elements: Vec<SceneElement<R>>,
        target: ImageDescription,
        (offset, scale): (Point<i32, Physical>, Scale<f64>),
    ) -> Vec<OutputElement<R>>
    where
        R: SceneRenderer,
        R::TextureId: Send + Clone + 'static,
    {
        let described = self.surface_descriptions();
        let from_srgb = Conversion::new(ImageDescription::SRGB, target);
        let moved =
            |element| RelocateRenderElement::from_element(element, offset, Relocate::Relative);
        elements
            .into_iter()
            .map(|element| match element {
                // A blur copies what is drawn: it is in the output's
                // colours already.
                SceneElement::Blur(_) => ColorElement::unconverted(moved(element)),
                // A solid colour is drawn without a texture: converted now.
                SceneElement::Solid(solid) if !from_srgb.is_identity() => {
                    ColorElement::unconverted(moved(SceneElement::Solid(
                        color_element::convert_solid(&solid, &from_srgb, scale),
                    )))
                }
                SceneElement::Surface(ref surface) => {
                    let conversion = described
                        .get(surface.id())
                        .map_or(from_srgb, |source| Conversion::new(*source, target));
                    ColorElement::new(moved(element), &conversion)
                }
                other @ (SceneElement::Panel(_)
                | SceneElement::Solid(_)
                | SceneElement::Shaped(_)
                | SceneElement::_GenericCatcher(_)) => ColorElement::new(moved(other), &from_srgb),
            })
            .collect()
    }

    /// Returns what the surfaces shown said their pixels mean, by their
    /// elements' ids: only those that said something other than sRGB.
    fn surface_descriptions(&self) -> HashMap<Id, ImageDescription> {
        let mut described = HashMap::new();
        let mut gather = |root: &WlSurface| {
            let trees = std::iter::once(root.clone()).chain(
                PopupManager::popups_for_surface(root).map(|(popup, _)| popup.wl_surface().clone()),
            );
            for tree in trees {
                with_surface_tree_downward(
                    &tree,
                    (),
                    |_, _, ()| TraversalAction::DoChildren(()),
                    |surface, states, ()| {
                        let description = color_management::surface_description(states);
                        if description != ImageDescription::SRGB {
                            described.insert(Id::from_wayland_resource(surface), description);
                        }
                    },
                    |_, _, ()| true,
                );
            }
        };
        for (_, toplevel) in self.windows() {
            if let Some(surface) = toplevel.wl_surface() {
                gather(&surface);
            }
        }
        self.layers
            .each(&ALL_LAYERS, |surface, _| gather(surface.wl_surface()));
        described
    }

    /// Returns what window `id` draws itself, at `scale`, with the corner
    /// of its content at the origin: no frame, nor anything over it, as a
    /// window shared to a stream shows. None for a window not there.
    pub(crate) fn window_elements<R>(
        &self,
        renderer: &mut R,
        id: WindowId,
        scale: Scale<f64>,
    ) -> Vec<OutputElement<R>>
    where
        R: SceneRenderer,
        R::TextureId: Send + Clone + 'static,
    {
        let (Some(window), Some(toplevel)) =
            (self.window_manager.window(id), self.toplevels.get(id))
        else {
            return Vec::new();
        };
        let offset = window.surface_origin() - window.content_area().loc;
        let elements: Vec<SceneElement<R>> = toplevel
            .render_elements(
                renderer,
                offset.to_physical_precise_round(scale),
                scale,
                1.0,
            )
            .into_iter()
            .map(SceneElement::Surface)
            .collect::<Vec<_>>();
        // A shared window is shown in sRGB, as a stream's viewers see it.
        self.in_colors_of(elements, ImageDescription::SRGB, (Point::default(), scale))
    }

    /// Returns what `output` (whose corner is at `corner`) shows while the
    /// session is locked: the pointer and the locker's surface; black
    /// until the locker gives one.
    fn lock_elements<R>(
        &self,
        renderer: &mut R,
        output: &Output,
        corner: Point<i32, Physical>,
        scale: Scale<f64>,
    ) -> Vec<SceneElement<R>>
    where
        R: SceneRenderer,
        R::TextureId: Send + Clone + 'static,
    {
        let mut elements = self
            .pointer
            .map(|(cursor, location)| cursor.render_elements(renderer, location, scale))
            .unwrap_or_default();
        if let Some(surface) = self.drawn.lock.surface_on(output) {
            let surface_elements: Vec<WaylandSurfaceRenderElement<R>> =
                render_elements_from_surface_tree(
                    renderer,
                    surface,
                    corner,
                    scale,
                    1.0,
                    Kind::Unspecified,
                );
            elements.extend(surface_elements.into_iter().map(SceneElement::Surface));
        }
        elements
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
        R: SceneRenderer,
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
                    toplevel
                        .wl_surface()
                        .and_then(|surface| {
                            BlurElement::beneath(&surface, origin, scale, self.drawn.blur)
                        })
                        .map(SceneElement::Blur),
                );
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
        R: SceneRenderer,
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

    /// Returns the taskbars shown: none while a fullscreen window covers
    /// them, nor those auto-hidden.
    fn shown_taskbars(&self) -> impl Iterator<Item = &'state Taskbar> {
        let is_covered = self.is_fullscreen_on_top();
        self.drawn
            .taskbars
            .iter()
            .filter(move |taskbar| taskbar.is_shown() && !is_covered)
    }

    /// Returns the topmost shown window's client surface or frame at
    /// `point`, among those in the above-dock layer when `is_above_dock`,
    /// else among the rest.
    fn window_at(&self, point: Point<f64, Logical>, is_above_dock: bool) -> Option<WindowTarget> {
        let metrics = self.window_manager.metrics();
        let windows = self.windows().filter(|(window, _)| {
            self.window_manager.is_above_dock(window.id()) == is_above_dock
                && self.passed_through != Some(window.id())
        });
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
                // The pointer is given relative to where the client thinks
                // its window is (an X11 window lags while it moves).
                let origin = (offset + toplevel.pointer_origin(surface_origin)).to_f64();
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

    /// Returns the locker's surface under `point`, while locked: its
    /// output's.
    fn lock_target_at(&self, point: Point<f64, Logical>) -> Option<PointerTarget> {
        let output = self.layers.outputs().at(point)?;
        let surface = self.drawn.lock.surface_on(output)?;
        let corner = Outputs::geometry(output).loc;
        let (surface, origin) = under_from_surface_tree(
            surface,
            point - corner.to_f64(),
            corner,
            WindowSurfaceType::ALL,
        )?;
        Some(PointerTarget::Lock {
            surface,
            origin: origin.to_f64(),
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
        R: SceneRenderer,
        R::TextureId: Send + Clone + 'static,
    {
        self.layers.each(layers, |surface, origin| {
            let origin = origin.to_physical_precise_round(scale);
            let surface_elements: Vec<WaylandSurfaceRenderElement<R>> =
                surface.render_elements(renderer, origin, scale, 1.0);
            elements.extend(surface_elements.into_iter().map(SceneElement::Surface));
            elements.extend(
                BlurElement::beneath(surface.wl_surface(), origin, scale, self.drawn.blur)
                    .map(SceneElement::Blur),
            );
        });
    }
}

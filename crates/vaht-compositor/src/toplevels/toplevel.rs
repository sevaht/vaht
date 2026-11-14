//! The client's side of a window: the only code that knows its protocol.
//!
//! A [`Toplevel`] translates both ways between the protocol and the window
//! manager's values: it reads a commit as a [`CommittedState`], and carries
//! out a [`RequestedState`] or an [`Ending`]. A toplevel is an xdg-shell
//! surface or an X11 window (through Xwayland); nothing outside this module
//! needs to know which (01 §4.1). The differences it hides: an X11 window
//! is told its position whenever it moves, its title and class are X
//! properties, and it is killed by the process its `_NET_WM_PID` names,
//! never by its connection's, which is Xwayland's.

use std::time::Duration;

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use smithay::backend::renderer::element::AsRenderElements;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{ImportAll, Renderer};
use smithay::desktop::space::SpaceElement as _;
use smithay::desktop::{Window as SurfaceWindow, WindowSurface, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource as _};
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus as _;
use smithay::wayland::shell::xdg::{SurfaceCachedState, ToplevelSurface, XdgToplevelSurfaceData};
use smithay::wayland::xdg_toplevel_icon::ToplevelIconCachedState;
use smithay::xwayland::X11Surface;

use crate::focus_target::FocusTarget;
use crate::outputs::Outputs;
use crate::window_manager::{
    Axes, ClientIcon, CommittedState, Ending, PlacementKind, RequestedState, SizeLimits,
};

/// The client's side of a window.
#[derive(Debug)]
pub(crate) struct Toplevel {
    /// Smithay's surface-agnostic window, which tracks the surface tree.
    window: SurfaceWindow,
    /// The icon picture last read, with the buffer it was read from, so an
    /// unchanged icon is not read again.
    read_icon: Option<(WlBuffer, vaht_ui::Icon)>,
    /// The area an X11 window was last told it covers, so it is told only
    /// when that changes.
    told_area: Option<Rectangle<i32, Logical>>,
}

impl Toplevel {
    /// Wraps a new xdg-shell toplevel.
    pub(crate) fn from_xdg(surface: ToplevelSurface) -> Self {
        Self {
            window: SurfaceWindow::new_wayland_window(surface),
            read_icon: None,
            told_area: None,
        }
    }

    /// Wraps an X11 window that asked to be mapped.
    pub(crate) fn from_x11(surface: X11Surface) -> Self {
        Self {
            window: SurfaceWindow::new_x11_window(surface),
            read_icon: None,
            told_area: None,
        }
    }

    /// Returns the root surface of the window; an X11 window has none
    /// until Xwayland pairs it with one.
    pub(crate) fn wl_surface(&self) -> Option<WlSurface> {
        self.window.wl_surface().map(std::borrow::Cow::into_owned)
    }

    /// Returns whether `surface` is this window's root surface.
    pub(crate) fn has_surface(&self, surface: &WlSurface) -> bool {
        self.window
            .wl_surface()
            .is_some_and(|own| own.as_ref() == surface)
    }

    /// Returns the X11 side of the window, if it is an X11 window.
    pub(crate) fn x11(&self) -> Option<&X11Surface> {
        self.window.x11_surface()
    }

    /// Returns what gets the keyboard when the window has the focus.
    pub(crate) fn focus_target(&self) -> FocusTarget {
        match self.window.underlying_surface() {
            WindowSurface::Wayland(toplevel) => FocusTarget::Surface(toplevel.wl_surface().clone()),
            WindowSurface::X11(surface) => FocusTarget::X11(Box::new(surface.clone())),
        }
    }

    /// Returns what the client last committed; `read` reads the picture
    /// in a buffer, for an icon the client drew.
    pub(crate) fn committed_state(
        &mut self,
        read: impl FnOnce(&WlBuffer) -> Option<vaht_ui::Icon>,
    ) -> CommittedState {
        if let Some(surface) = self.x11() {
            // An X11 window's class is what Fluxbox and its rules match,
            // as a Wayland window's app id is.
            return CommittedState {
                title: surface.title(),
                app_id: surface.class(),
                icon: None,
                geometry: self.window.geometry(),
                size_limits: self.size_limits(),
                is_self_decorated: surface.is_decorated(),
            };
        }
        let Some(root) = self.wl_surface() else {
            return CommittedState::default();
        };
        let (title, app_id) = self
            .with_attributes(|attributes| {
                (
                    attributes.title.clone().unwrap_or_default(),
                    attributes.app_id.clone().unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        let (icon_name, icon_buffer) = with_states(&root, |states| {
            let mut cached = states.cached_state.get::<ToplevelIconCachedState>();
            let icon = cached.current();
            (
                icon.icon_name().map(str::to_owned),
                Self::largest_buffer(icon.buffers()),
            )
        });
        // A name scales with the theme, so it comes before a picture.
        let icon = match (icon_name, icon_buffer) {
            (Some(name), _) => Some(ClientIcon::Named(name)),
            (None, Some(buffer)) => self.icon_in(&buffer, read).map(ClientIcon::Pixels),
            (None, None) => None,
        };
        CommittedState {
            title,
            app_id,
            icon,
            geometry: self.window.geometry(),
            size_limits: self.size_limits(),
            // vaht draws every Wayland window's frame (xdg-decoration).
            is_self_decorated: false,
        }
    }

    /// Returns the picture in `buffer`: the one read before, if it is the
    /// same buffer, else what `read` reads now.
    fn icon_in(
        &mut self,
        buffer: &WlBuffer,
        read: impl FnOnce(&WlBuffer) -> Option<vaht_ui::Icon>,
    ) -> Option<vaht_ui::Icon> {
        if let Some((known, icon)) = &self.read_icon
            && known == buffer
        {
            return Some(icon.clone());
        }
        let icon = read(buffer)?;
        self.read_icon = Some((buffer.clone(), icon.clone()));
        Some(icon)
    }

    /// Returns the widest of a client's icon `buffers` (each with its
    /// scale), which reads best scaled down.
    fn largest_buffer(buffers: &[(WlBuffer, i32)]) -> Option<WlBuffer> {
        buffers
            .iter()
            .max_by_key(|(buffer, scale)| {
                // Only the description is read here, never the memory.
                let width =
                    smithay::wayland::shm::with_buffer_contents(buffer, |_, _, data| data.width)
                        .unwrap_or(0);
                (width, *scale)
            })
            .map(|(buffer, _)| buffer.clone())
    }

    /// Returns the surface under `point` (relative to the surface origin) and
    /// that surface's offset from the origin.
    pub(crate) fn surface_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        self.window.surface_under(point, WindowSurfaceType::ALL)
    }

    /// Returns the elements that draw the window and its popups, with the
    /// surface origin at `origin`, at opacity `alpha` (0.0 to 1.0).
    pub(crate) fn render_elements<R>(
        &self,
        renderer: &mut R,
        origin: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<WaylandSurfaceRenderElement<R>>
    where
        R: Renderer + ImportAll,
        R::TextureId: Clone + 'static,
    {
        self.window.render_elements(renderer, origin, scale, alpha)
    }

    /// Lets the client know a frame including it was shown on `output`.
    pub(crate) fn send_frame(&self, output: &Output, time: Duration) {
        self.window
            .send_frame(output, time, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
    }

    /// Tells the client which of `outputs` show its window, whose content
    /// covers `area` (global coordinates) with its surface at `origin`.
    pub(crate) fn enter_outputs(
        &self,
        outputs: &Outputs,
        area: Rectangle<i32, Logical>,
        origin: Point<i32, Logical>,
    ) {
        for output in outputs.iter() {
            match Outputs::geometry(output).intersection(area) {
                Some(mut overlap) => {
                    overlap.loc -= origin;
                    self.window.output_enter(output, overlap);
                }
                None => self.window.output_leave(output),
            }
        }
    }

    /// Updates what is known about the surface tree after a commit.
    pub(crate) fn on_commit(&self) {
        self.window.on_commit();
    }

    /// Sends the first configure, which a client waits for before drawing.
    pub(crate) fn configure_if_unconfigured(&self) {
        if let Some(xdg) = self.xdg()
            && !xdg.is_initial_configure_sent()
        {
            xdg.send_configure();
        }
    }

    /// Tells an X11 window that its content now covers `area` (global
    /// coordinates), unless it was told so last: an X11 program places its
    /// menus by where it thinks its window is. A Wayland window needs no
    /// telling.
    pub(crate) fn place(&mut self, area: Rectangle<i32, Logical>) {
        let Some(surface) = self.x11() else {
            return;
        };
        if self.told_area == Some(area) || area.size.w <= 0 || area.size.h <= 0 {
            return;
        }
        if let Err(error) = surface.configure(area) {
            tracing::debug!(%error, "an X11 window could not be told where it is");
            return;
        }
        self.told_area = Some(area);
    }

    /// Asks the client to make its window as `requested` says; an X11
    /// window's content is at `location` (global coordinates).
    pub(crate) fn request(&mut self, requested: &RequestedState, location: Point<i32, Logical>) {
        if let Some(surface) = self.x11().cloned() {
            Self::request_x11(&surface, requested);
            let size = requested.size.unwrap_or_else(|| surface.geometry().size);
            self.place(Rectangle::new(location, size));
            return;
        }
        let Some(xdg) = self.xdg() else {
            return;
        };
        let (maximized, fullscreen, tiled) = match requested.placement {
            PlacementKind::Floating => (false, false, [false; 4]),
            PlacementKind::Maximized(axes) => {
                let horizontal = matches!(axes, Axes::Horizontal | Axes::Both);
                let vertical = matches!(axes, Axes::Vertical | Axes::Both);
                (
                    axes == Axes::Both,
                    false,
                    [horizontal, horizontal, vertical, vertical],
                )
            }
            PlacementKind::Tiled => (false, false, [true; 4]),
            PlacementKind::Fullscreen => (false, true, [false; 4]),
        };
        let tiled_states = [
            xdg_toplevel::State::TiledLeft,
            xdg_toplevel::State::TiledRight,
            xdg_toplevel::State::TiledTop,
            xdg_toplevel::State::TiledBottom,
        ];
        xdg.with_pending_state(|state| {
            let mut set = |flag: xdg_toplevel::State, is_on: bool| {
                if is_on {
                    state.states.set(flag);
                } else {
                    state.states.unset(flag);
                }
            };
            set(xdg_toplevel::State::Maximized, maximized);
            set(xdg_toplevel::State::Fullscreen, fullscreen);
            for (flag, is_on) in tiled_states.into_iter().zip(tiled) {
                set(flag, is_on);
            }
            set(xdg_toplevel::State::Activated, requested.is_activated);
            set(xdg_toplevel::State::Resizing, requested.is_resizing);
            state.size = requested.size;
        });
        self.send_configure_if_ready();
    }

    /// Tells an X11 window the states `requested` asks for: maximized,
    /// fullscreen, focused.
    fn request_x11(surface: &X11Surface, requested: &RequestedState) {
        let (maximized, fullscreen) = match requested.placement {
            PlacementKind::Maximized(axes) => (axes == Axes::Both, false),
            PlacementKind::Fullscreen => (false, true),
            PlacementKind::Floating | PlacementKind::Tiled => (false, false),
        };
        let results = [
            surface.set_maximized(maximized),
            surface.set_fullscreen(fullscreen),
            surface.set_activated(requested.is_activated),
        ];
        if let Some(Err(error)) = results.into_iter().find(Result::is_err) {
            tracing::debug!(%error, "an X11 window could not be told its state");
        }
    }

    /// Answers a request the window manager did not act on: xdg-shell
    /// requires a configure in reply even when nothing changes.
    pub(crate) fn acknowledge(&self) {
        self.send_configure_if_ready();
    }

    /// Ends the window as `ending` says.
    pub(crate) fn end(&self, ending: Ending, display: &DisplayHandle) {
        match (ending, self.window.underlying_surface()) {
            (Ending::Close, WindowSurface::Wayland(xdg)) => xdg.send_close(),
            (Ending::Close, WindowSurface::X11(surface)) => {
                if let Err(error) = surface.close() {
                    tracing::warn!(%error, "an X11 window could not be asked to close");
                }
            }
            (Ending::Kill, _) => self.kill_program(display),
        }
    }

    /// Returns the process that owns the window: for an X11 window, the
    /// one its `_NET_WM_PID` names; for a Wayland window, the one the
    /// kernel says is at the other end of the client's socket.
    fn program(&self, display: &DisplayHandle) -> Option<i32> {
        if let Some(surface) = self.x11() {
            return surface
                .pid()
                .and_then(|pid| i32::try_from(pid).ok())
                .or_else(|| {
                    tracing::warn!("an X11 window names no process (_NET_WM_PID)");
                    None
                });
        }
        let client = display.get_client(self.wl_surface()?.id()).ok()?;
        match client.get_credentials(display) {
            Ok(credentials) => Some(credentials.pid),
            Err(error) => {
                tracing::warn!(%error, "could not learn which program owns a window");
                None
            }
        }
    }

    /// Ends the window's program at once (SIGKILL), as Fluxbox's `Kill`
    /// does. A window of vaht's own is never killed.
    fn kill_program(&self, display: &DisplayHandle) {
        let Some(pid) = self.program(display) else {
            return;
        };
        if pid <= 0 || u32::try_from(pid).is_ok_and(|pid| pid == std::process::id()) {
            tracing::warn!(pid, "not killing the program of a window of vaht's own");
            return;
        }
        tracing::info!(pid, "killing a window's program");
        if let Err(error) = signal::kill(Pid::from_raw(pid), Signal::SIGKILL) {
            tracing::warn!(%error, pid, "could not kill a window's program");
        }
    }

    /// Returns the content sizes the client accepts.
    fn size_limits(&self) -> SizeLimits {
        if let Some(surface) = self.x11() {
            return SizeLimits::from_client(
                surface.min_size().unwrap_or_default(),
                surface.max_size().unwrap_or_default(),
            );
        }
        let Some(root) = self.wl_surface() else {
            return SizeLimits::default();
        };
        with_states(&root, |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            SizeLimits::from_client(current.min_size, current.max_size)
        })
    }

    /// Sends pending changes, once the client has had its first configure;
    /// before that, they go out with it.
    fn send_configure_if_ready(&self) {
        if let Some(xdg) = self.xdg()
            && xdg.is_initial_configure_sent()
        {
            xdg.send_pending_configure();
        }
    }

    /// Returns the xdg-shell side of the window, if it is a Wayland window.
    fn xdg(&self) -> Option<&ToplevelSurface> {
        match self.window.underlying_surface() {
            WindowSurface::Wayland(toplevel) => Some(toplevel),
            WindowSurface::X11(_) => None,
        }
    }

    /// Reads the toplevel's attributes (title, app id).
    fn with_attributes<T>(
        &self,
        read: impl FnOnce(&smithay::wayland::shell::xdg::XdgToplevelSurfaceRoleAttributes) -> T,
    ) -> Option<T> {
        let root = self.wl_surface()?;
        with_states(&root, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|attributes| {
                    read(
                        &attributes
                            .lock()
                            .expect("no thread panics while holding toplevel attributes"),
                    )
                })
        })
    }
}

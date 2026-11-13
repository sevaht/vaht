//! The client's side of a window: the only code that knows its protocol.
//!
//! A [`Toplevel`] translates both ways between the protocol and the window
//! manager's values: it reads a commit as a [`CommittedState`], and carries
//! out a [`RequestedState`] or an [`Ending`]. Today every toplevel is an
//! xdg-shell surface; X11 windows will become a second case here, and
//! nothing outside this module will need to know which one a window is
//! (01 §4.1).

use std::time::Duration;

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use smithay::backend::renderer::element::AsRenderElements;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{ImportAll, Renderer};
use smithay::desktop::{Window as SurfaceWindow, WindowSurface, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource as _};
use smithay::utils::{Logical, Physical, Point, Scale};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::{SurfaceCachedState, ToplevelSurface, XdgToplevelSurfaceData};
use smithay::wayland::xdg_toplevel_icon::ToplevelIconCachedState;

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
}

impl Toplevel {
    /// Wraps a new xdg-shell toplevel.
    pub(crate) fn from_xdg(surface: ToplevelSurface) -> Self {
        Self {
            window: SurfaceWindow::new_wayland_window(surface),
            read_icon: None,
        }
    }

    /// Returns the root surface of the window.
    pub(crate) fn wl_surface(&self) -> &WlSurface {
        self.xdg().wl_surface()
    }

    /// Returns whether `surface` is this window's root surface.
    pub(crate) fn has_surface(&self, surface: &WlSurface) -> bool {
        self.wl_surface() == surface
    }

    /// Returns what the client last committed; `read` reads the picture
    /// in a buffer, for an icon the client drew.
    pub(crate) fn committed_state(
        &mut self,
        read: impl FnOnce(&WlBuffer) -> Option<vaht_ui::Icon>,
    ) -> CommittedState {
        let (title, app_id) = self
            .with_attributes(|attributes| {
                (
                    attributes.title.clone().unwrap_or_default(),
                    attributes.app_id.clone().unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        let (icon_name, icon_buffer) = with_states(self.wl_surface(), |states| {
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

    /// Updates what is known about the surface tree after a commit.
    pub(crate) fn on_commit(&self) {
        self.window.on_commit();
    }

    /// Sends the first configure, which a client waits for before drawing.
    pub(crate) fn configure_if_unconfigured(&self) {
        if !self.xdg().is_initial_configure_sent() {
            self.xdg().send_configure();
        }
    }

    /// Asks the client to make its window as `requested` says.
    pub(crate) fn request(&self, requested: &RequestedState) {
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
        self.xdg().with_pending_state(|state| {
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

    /// Answers a request the window manager did not act on: xdg-shell
    /// requires a configure in reply even when nothing changes.
    pub(crate) fn acknowledge(&self) {
        self.send_configure_if_ready();
    }

    /// Ends the window as `ending` says.
    pub(crate) fn end(&self, ending: Ending, display: &DisplayHandle) {
        match ending {
            Ending::Close => self.xdg().send_close(),
            Ending::Kill => self.kill_program(display),
        }
    }

    /// Ends the client's program at once (SIGKILL), as Fluxbox's `Kill`
    /// does: the program is the one the kernel says is at the other end of
    /// the client's socket. A window of vaht's own is never killed.
    fn kill_program(&self, display: &DisplayHandle) {
        let Ok(client) = display.get_client(self.wl_surface().id()) else {
            return;
        };
        let pid = match client.get_credentials(display) {
            Ok(credentials) => credentials.pid,
            Err(error) => {
                tracing::warn!(%error, "could not learn which program owns a window");
                return;
            }
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
        with_states(self.wl_surface(), |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            SizeLimits::from_client(current.min_size, current.max_size)
        })
    }

    /// Sends pending changes, once the client has had its first configure;
    /// before that, they go out with it.
    fn send_configure_if_ready(&self) {
        if self.xdg().is_initial_configure_sent() {
            self.xdg().send_pending_configure();
        }
    }

    /// Returns the xdg-shell side of the window.
    fn xdg(&self) -> &ToplevelSurface {
        match self.window.underlying_surface() {
            WindowSurface::Wayland(toplevel) => toplevel,
        }
    }

    /// Reads the toplevel's attributes (title, app id).
    fn with_attributes<T>(
        &self,
        read: impl FnOnce(&smithay::wayland::shell::xdg::XdgToplevelSurfaceRoleAttributes) -> T,
    ) -> Option<T> {
        with_states(self.wl_surface(), |states| {
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

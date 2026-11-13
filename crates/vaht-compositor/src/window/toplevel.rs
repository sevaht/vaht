//! The client's side of a window: the only code that knows its protocol.
//!
//! Everything else talks to a [`Toplevel`] in protocol-neutral terms (its
//! size, title, activation, closing). Today every toplevel is an xdg-shell
//! surface; X11 windows will become a second case here, and nothing outside
//! this module will need to know which one a window is (01 §4.1).

use std::time::Duration;

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use smithay::backend::renderer::element::AsRenderElements;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{ImportAll, Renderer};
use smithay::desktop::{Window as SurfaceWindow, WindowSurface, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource as _};
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::{SurfaceCachedState, ToplevelSurface, XdgToplevelSurfaceData};

use super::resize::SizeLimits;

/// The client's side of a window.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Toplevel {
    /// Smithay's surface-agnostic window, which tracks the surface tree.
    window: SurfaceWindow,
}

impl Toplevel {
    /// Wraps a new xdg-shell toplevel.
    pub(crate) fn from_xdg(surface: ToplevelSurface) -> Self {
        Self {
            window: SurfaceWindow::new_wayland_window(surface),
        }
    }

    /// Returns the root surface of the window.
    pub(crate) fn wl_surface(&self) -> &WlSurface {
        self.xdg().wl_surface()
    }

    /// Returns the window's content area relative to its surface: the
    /// surface minus any client-side shadow.
    pub(crate) fn geometry(&self) -> Rectangle<i32, Logical> {
        self.window.geometry()
    }

    /// Returns the window's title, if the client set one.
    pub(crate) fn title(&self) -> Option<String> {
        self.with_attributes(|attributes| attributes.title.clone())
            .flatten()
    }

    /// Returns the client's application id, if it set one.
    pub(crate) fn app_id(&self) -> Option<String> {
        self.with_attributes(|attributes| attributes.app_id.clone())
            .flatten()
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
    /// surface origin at `origin`.
    pub(crate) fn render_elements<R>(
        &self,
        renderer: &mut R,
        origin: Point<i32, Physical>,
        scale: Scale<f64>,
    ) -> Vec<WaylandSurfaceRenderElement<R>>
    where
        R: Renderer + ImportAll,
        R::TextureId: Clone + 'static,
    {
        self.window.render_elements(renderer, origin, scale, 1.0)
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

    /// Shows the window as focused or not; the client restyles itself.
    pub(crate) fn set_activated(&self, is_activated: bool) {
        if self.window.set_activated(is_activated) {
            self.xdg().send_pending_configure();
        }
    }

    /// Returns the content sizes the client accepts.
    pub(crate) fn size_limits(&self) -> SizeLimits {
        with_states(self.wl_surface(), |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            SizeLimits::from_client(current.min_size, current.max_size)
        })
    }

    /// Asks the client to make its content `size`.
    pub(crate) fn request_size(&self, size: Size<i32, Logical>) {
        self.xdg()
            .with_pending_state(|state| state.size = Some(size));
        self.send_configure_if_ready();
    }

    /// Tells the client whether the user is resizing it, so it can redraw
    /// cheaply meanwhile.
    pub(crate) fn set_resizing(&self, is_resizing: bool) {
        self.xdg().with_pending_state(|state| {
            if is_resizing {
                state.states.set(xdg_toplevel::State::Resizing);
            } else {
                state.states.unset(xdg_toplevel::State::Resizing);
            }
        });
        self.send_configure_if_ready();
    }

    /// Ends the client's program at once (SIGKILL), as Fluxbox's `Kill`
    /// does: the program is the one the kernel says is at the other end of
    /// the client's socket. A window of vaht's own is never killed.
    pub(crate) fn kill_program(&self, display: &DisplayHandle) {
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

    /// Asks the client to close the window.
    pub(crate) fn close(&self) {
        self.xdg().send_close();
    }

    /// Returns whether `surface` is this window's root surface.
    pub(crate) fn has_surface(&self, surface: &WlSurface) -> bool {
        self.wl_surface() == surface
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

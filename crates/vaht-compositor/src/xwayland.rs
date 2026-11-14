//! X11 programs, through Xwayland.
//!
//! vaht starts Xwayland once at start ([`XwaylandServer::start`]) and
//! becomes its window manager when it is ready: programs started from then
//! on find it in `DISPLAY`. A managed X11 window is a window like any other
//! (its [`crate::toplevels::Toplevel`] hides the protocol); an
//! override-redirect window (an X program's menu, tooltip or drop-down)
//! manages itself, and shows over the windows where it says
//! ([`XwaylandServer::unmanaged`]). The clipboard and primary selection
//! pass both ways between X and Wayland programs.

mod shape;

use std::os::fd::OwnedFd;
use std::path::Path;
use std::process::Stdio;

use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::selection::data_device::{
    clear_data_device_selection, current_data_device_selection_userdata,
    request_data_device_client_selection, set_data_device_selection,
};
use smithay::wayland::selection::primary_selection::{
    clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection,
    set_primary_selection,
};
use smithay::wayland::xwayland_keyboard_grab::XWaylandKeyboardGrabHandler;
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge, WmWindowProperty, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
use x11rb::wrapper::ConnectionExt as _;

use self::shape::ShapeTracker;
pub(crate) use self::shape::WindowShape;
use crate::Compositor;
use crate::event_source::EventSource;
use crate::focus_target::FocusTarget;
use crate::resources::ResourceLedger;
use crate::window_manager::{ClientRequest, HorizontalEdge, ResizeEdges, VerticalEdge, WindowDrag};

/// Xwayland, and vaht as its window manager.
#[derive(Debug)]
pub(crate) struct XwaylandServer {
    /// The window manager's connection, once Xwayland is ready.
    wm: Option<X11Wm>,
    /// The X display Xwayland serves (`:0`…), once it is ready.
    display_number: Option<u32>,
    /// The override-redirect windows mapped, bottom first.
    unmanaged: Vec<X11Surface>,
    /// Every window's shape, once Xwayland is ready.
    shapes: Option<ShapeTracker>,
    /// Xwayland's readiness, registered while the server lives.
    _source: EventSource<Compositor>,
}

impl XwaylandServer {
    /// Starts Xwayland for `display`; it is ready a moment later, which
    /// [`Compositor::xwayland_ready`] hears.
    ///
    /// # Errors
    ///
    /// Returns why Xwayland could not be started (it is not installed).
    pub(crate) fn start(
        handle: &LoopHandle<'static, Compositor>,
        display: &DisplayHandle,
        ledger: &ResourceLedger,
        input_emulation: Option<&Path>,
    ) -> Result<Self, String> {
        // XTest input goes to vaht's EIS socket, if it serves one.
        let environment = input_emulation.map(|socket| {
            (
                "LIBEI_SOCKET".to_owned(),
                socket.to_string_lossy().into_owned(),
            )
        });
        let (xwayland, client) = XWayland::spawn(
            display,
            None,
            environment,
            std::iter::empty::<String>(),
            true,
            Stdio::null(),
            Stdio::null(),
            |_| (),
        )
        .map_err(|error| format!("Xwayland did not start: {error}"))?;
        let source =
            EventSource::insert(
                handle,
                ledger,
                xwayland,
                move |event, (), compositor| match event {
                    XWaylandEvent::Ready {
                        x11_socket,
                        display_number,
                    } => compositor.xwayland_ready(x11_socket, display_number, client.clone()),
                    XWaylandEvent::Error => tracing::warn!("Xwayland stopped before it was ready"),
                },
            )
            .map_err(|error| format!("Xwayland's events cannot be registered: {}", error.error))?;
        Ok(Self {
            wm: None,
            display_number: None,
            unmanaged: Vec::new(),
            shapes: None,
            _source: source,
        })
    }

    /// Becomes the window manager of the ready server on `display_number`,
    /// and follows its windows' shapes over a connection of vaht's own.
    pub(crate) fn manage(
        &mut self,
        wm: X11Wm,
        display_number: u32,
        (handle, ledger): (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        self.wm = Some(wm);
        self.display_number = Some(display_number);
        let display = format!(":{display_number}");
        self.shapes = ShapeTracker::connect(&display, handle, ledger)
            .inspect_err(|error| {
                tracing::warn!(
                    error,
                    "X11 windows' shapes are not followed; shaped windows show whole"
                );
            })
            .ok();
        tracing::info!(display = display_number, "Xwayland is ready");
    }

    /// Stops following the shape of `window`, unmapped or gone.
    pub(crate) fn untrack_shape(&mut self, window: &X11Surface) {
        if let Some(shapes) = &mut self.shapes {
            shapes.untrack(window.window_id());
        }
    }

    /// Loads the X resources `resources` (an `.Xresources` file's text)
    /// into the server on `display`: the root window's `RESOURCE_MANAGER`,
    /// which X programs read their settings from, as xrdb sets it.
    ///
    /// # Errors
    ///
    /// Returns why not: the server cannot be reached or refused.
    pub(crate) fn load_resources(display: &str, resources: &[u8]) -> Result<(), String> {
        let (connection, screen) = x11rb::connect(Some(display))
            .map_err(|error| format!("cannot reach {display}: {error}"))?;
        let root = connection.setup().roots[screen].root;
        connection
            .change_property8(
                PropMode::REPLACE,
                root,
                AtomEnum::RESOURCE_MANAGER,
                AtomEnum::STRING,
                resources,
            )
            .map_err(|error| error.to_string())?;
        // A round trip: the property is set before the connection closes.
        connection
            .get_input_focus()
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Starts following the shape of `window`, just mapped.
    fn track_shape(&mut self, window: &X11Surface) {
        if let Some(shapes) = &mut self.shapes {
            shapes.track(window.window_id(), window.last_configure().size);
        }
    }

    /// Tells X programs that a Wayland client now holds `selection`,
    /// offering `mime_types`, or that it was cleared (none), so X programs
    /// paste it.
    pub(crate) fn offer_selection(
        &mut self,
        selection: SelectionTarget,
        mime_types: Option<Vec<String>>,
    ) {
        let Some(wm) = &mut self.wm else {
            return;
        };
        // Smithay queues the change of owner without sending it, so an X
        // program pasting before the window manager next writes would find
        // no owner. A stacking update of no windows writes nothing more and
        // sends what is queued.
        let offered = wm
            .new_selection(selection, mime_types)
            .map_err(|error| error.to_string());
        let sent = offered.and_then(|()| {
            wm.update_stacking_order_upwards(std::iter::empty::<&X11Surface>())
                .map_err(|error| error.to_string())
        });
        if let Err(error) = sent {
            tracing::debug!(
                error,
                ?selection,
                "X programs could not be offered the selection"
            );
        }
    }

    /// Asks the X program holding `selection` to write it as `mime_type`
    /// to `fd`, which a Wayland client reads.
    pub(crate) fn give_selection(
        &mut self,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        if let Some(wm) = &mut self.wm
            && let Err(error) = wm.send_selection(selection, mime_type, fd)
        {
            tracing::debug!(%error, ?selection, "an X program's selection could not be read");
        }
    }

    /// Returns the shape of `window`, if it is shaped.
    pub(crate) fn shape_of(&self, window: &X11Surface) -> Option<&WindowShape> {
        self.shapes.as_ref()?.shape_of(window.window_id())
    }

    /// Reads the shapes that changed; returns whether any did. `size_of`
    /// gives an X window's size by its id.
    pub(crate) fn follow_shapes(
        &mut self,
        size_of: impl Fn(u32) -> Option<smithay::utils::Size<i32, Logical>>,
    ) -> bool {
        self.shapes
            .as_mut()
            .is_some_and(|shapes| shapes.follow_changes(size_of))
    }

    /// Returns the X display programs connect to, such as `:0`, once
    /// Xwayland is ready.
    pub(crate) fn display_name(&self) -> Option<String> {
        self.display_number.map(|number| format!(":{number}"))
    }

    /// Returns the override-redirect windows shown, topmost first.
    pub(crate) fn unmanaged(&self) -> impl Iterator<Item = &X11Surface> {
        self.unmanaged.iter().rev()
    }

    /// Shows the override-redirect window `window` on top.
    fn show_unmanaged(&mut self, window: X11Surface) {
        self.track_shape(&window);
        self.unmanaged.retain(|known| *known != window);
        self.unmanaged.push(window);
    }

    /// Forgets the override-redirect window `window`; returns whether it
    /// was one.
    pub(crate) fn forget_unmanaged(&mut self, window: &X11Surface) -> bool {
        self.untrack_shape(window);
        let count = self.unmanaged.len();
        self.unmanaged.retain(|known| known != window);
        self.unmanaged.len() != count
    }
}

impl XWaylandShellHandler for Compositor {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.protocols.xwayland_shell
    }
}

/// An X program grabbing the keyboard (`XGrabKeyboard`, as games and
/// virtual machines do), which Xwayland passes on: honored for the window
/// with the keyboard, until vaht moves focus. vaht's bindings still see
/// each key first.
impl XWaylandKeyboardGrabHandler for Compositor {
    fn keyboard_focus_for_xsurface(&self, surface: &WlSurface) -> Option<FocusTarget> {
        self.x11_keyboard_grab_target(surface)
    }
}

impl XwmHandler for Compositor {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwayland
            .as_mut()
            .and_then(|server| server.wm.as_mut())
            .expect("Smithay asks for the window manager only once it runs")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Err(error) = window.set_mapped(true) {
            tracing::warn!(%error, "an X11 window could not be mapped");
            return;
        }
        if let Some(server) = &mut self.xwayland {
            server.track_shape(&window);
        }
        self.add_x11_window(window);
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(server) = &mut self.xwayland {
            server.show_unmanaged(window);
        }
        self.schedule_render();
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.forget_x11_window(&window);
        if !window.is_override_redirect()
            && let Err(error) = window.set_mapped(false)
        {
            tracing::debug!(%error, "an X11 window could not be unmapped");
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.forget_x11_window(&window);
    }

    #[expect(
        clippy::renamed_function_params,
        reason = "Smithay names the size w and h; vaht spells names out (N1)"
    )]
    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let current = window.last_configure();
        let to_i32 = |value: u32| i32::try_from(value).ok();
        let asked = Rectangle::<i32, Logical>::new(
            (x.unwrap_or(current.loc.x), y.unwrap_or(current.loc.y)).into(),
            (
                width.and_then(to_i32).unwrap_or(current.size.w),
                height.and_then(to_i32).unwrap_or(current.size.h),
            )
                .into(),
        );
        self.answer_x11_configure(&window, asked);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        if window.is_override_redirect() {
            self.schedule_render();
        } else {
            self.x11_position_confirmed(&window, geometry.loc);
        }
    }

    fn property_notify(&mut self, _xwm: XwmId, window: X11Surface, _property: WmWindowProperty) {
        // A title, class or decorations hint changed.
        self.x11_properties_changed(&window);
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        self.honor_x11_request(&window, ClientRequest::Maximize);
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        self.honor_x11_request(&window, ClientRequest::Unmaximize);
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        self.honor_x11_request(&window, ClientRequest::Fullscreen);
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        self.honor_x11_request(&window, ClientRequest::Unfullscreen);
    }

    fn minimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        self.honor_x11_request(&window, ClientRequest::Minimize);
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        _button: u32,
        resize_edge: ResizeEdge,
    ) {
        let (horizontal, vertical) = match resize_edge {
            ResizeEdge::Top => (HorizontalEdge::None, VerticalEdge::Top),
            ResizeEdge::Bottom => (HorizontalEdge::None, VerticalEdge::Bottom),
            ResizeEdge::Left => (HorizontalEdge::Left, VerticalEdge::None),
            ResizeEdge::Right => (HorizontalEdge::Right, VerticalEdge::None),
            ResizeEdge::TopLeft => (HorizontalEdge::Left, VerticalEdge::Top),
            ResizeEdge::TopRight => (HorizontalEdge::Right, VerticalEdge::Top),
            ResizeEdge::BottomLeft => (HorizontalEdge::Left, VerticalEdge::Bottom),
            ResizeEdge::BottomRight => (HorizontalEdge::Right, VerticalEdge::Bottom),
        };
        let edges = ResizeEdges {
            horizontal,
            vertical,
        };
        self.start_x11_drag(&window, WindowDrag::Resize { edges });
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        self.start_x11_drag(&window, WindowDrag::Move);
    }

    fn active_window_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        _timestamp: u32,
        currently_active_window: Option<X11Surface>,
    ) {
        self.activate_x11_window(&window, currently_active_window.as_ref());
    }

    fn allow_selection_access(&mut self, _xwm: XwmId, _selection: SelectionTarget) -> bool {
        // Only the X program with the keyboard reads the clipboard, as
        // only the focused Wayland client does.
        matches!(self.keyboard_focus, Some(FocusTarget::X11(_)))
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        let result = match selection {
            SelectionTarget::Clipboard => {
                request_data_device_client_selection(&self.seat, mime_type, fd)
                    .map_err(|error| error.to_string())
            }
            SelectionTarget::Primary => request_primary_client_selection(&self.seat, mime_type, fd)
                .map_err(|error| error.to_string()),
        };
        if let Err(error) = result {
            tracing::debug!(error, "an X program could not read the selection");
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&self.display_handle, &self.seat, mime_types, ());
            }
            SelectionTarget::Primary => {
                set_primary_selection(&self.display_handle, &self.seat, mime_types, ());
            }
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.display_handle, &self.seat);
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.display_handle, &self.seat);
                }
            }
        }
    }

    fn disconnected(&mut self, _xwm: XwmId) {
        tracing::warn!("Xwayland's window manager connection closed");
        if let Some(server) = &mut self.xwayland {
            server.wm = None;
            server.unmanaged.clear();
        }
    }
}

//! The compositor's state, and the loop that runs it.
//!
//! [`Compositor`] is the state Smithay's handler traits are implemented on.
//! It holds the subsystems and coordinates use cases that span them (a
//! window opening, focus moving); the behavior itself lives in the
//! subsystems' own types (01 §3).
//!
//! The Wayland [`Display`] is deliberately *not* part of that state. Clients
//! are dispatched with `display.dispatch_clients(&mut compositor)`, which
//! needs both mutably at once; keeping them apart makes that an ordinary
//! borrow, where Smithay's examples reach the display through `unsafe`
//! (R-X3). The event loop only watches a duplicate of the display's
//! descriptor to wake up, and clients are dispatched after every wake-up.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::os::fd::AsFd as _;
use std::sync::Arc;
use std::time::Instant;

use smithay::desktop::{PopupKind, PopupManager};
use smithay::input::keyboard::XkbConfig;
use smithay::input::{Seat, SeatState};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle, Resource as _};
use smithay::utils::{Logical, Point, Rectangle, Serial};
use smithay::wayland::compositor::{get_parent, is_sync_subsurface};
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::socket::ListeningSocketSource;
use vaht_ui::{Theme, Typesetter};

use crate::backend::{Backend, BackendChoice, BackendError};
use crate::capture::ScreenCapture;
use crate::client::ClientState;
use crate::event_source::EventSource;
use crate::protocols::ProtocolStates;
use crate::resources::{ProcessUsage, ResourceLedger};
use crate::termination::TerminationSignals;
use crate::window::{FrameGeometry, Toplevel, WindowId};
use crate::windows::Windows;

/// The running compositor.
pub struct Compositor {
    /// Issues ids, creates globals and inserts clients.
    pub(crate) display_handle: DisplayHandle,
    /// Counts every tracked resource.
    pub(crate) ledger: ResourceLedger,
    /// Smithay's state for each protocol global.
    pub(crate) protocols: ProtocolStates,
    /// The one seat: its keyboard and pointer.
    pub(crate) seat: Seat<Self>,
    /// Where frames go and input comes from.
    pub(crate) backend: Backend,
    /// Every managed window.
    pub(crate) windows: Windows,
    /// Popups, by the surface they belong to.
    pub(crate) popups: PopupManager,
    /// How everything vaht draws looks.
    pub(crate) theme: Theme,
    /// Draws screenshots for capture clients.
    pub(crate) capture: ScreenCapture,
    /// Shapes and draws text; shared by every panel.
    typesetter: Typesetter,
    /// The window whose titlebar the pointer is holding down, if any.
    pub(crate) held_titlebar: Option<WindowId>,
    /// The window with keyboard focus.
    focused: Option<WindowId>,
    /// When vaht started; frame times are measured from it.
    pub(crate) start_time: Instant,
    /// False once vaht has been asked to stop.
    is_running: bool,
    /// The name of the socket clients connect to (`WAYLAND_DISPLAY`).
    socket_name: OsString,
    /// The listening socket, the display's wake-up descriptor, and the
    /// termination signals.
    _event_sources: [EventSource<Self>; 3],
}

impl Compositor {
    /// How long a held key waits before repeating, in milliseconds.
    const KEY_REPEAT_DELAY_MILLISECONDS: i32 = 200;

    /// How often a held key repeats, per second.
    const KEY_REPEAT_RATE: i32 = 25;

    /// How far each new window is offset from the previous one.
    const CASCADE_STEP: i32 = 32;

    /// How many steps the cascade takes before starting over at the corner.
    const CASCADE_LENGTH: usize = 10;

    /// Runs vaht until it is asked to stop, by its host window closing or by
    /// one of the `termination` signals.
    ///
    /// # Errors
    ///
    /// Returns an error if vaht cannot start, or if waiting for events fails.
    pub fn run(options: RunOptions, termination: TerminationSignals) -> Result<(), RunError> {
        let mut event_loop: EventLoop<'static, Self> =
            EventLoop::try_new().map_err(RunError::EventLoop)?;
        let mut display: Display<Self> = Display::new().map_err(RunError::Display)?;
        let mut compositor = Self::new(&event_loop.handle(), &display, options, termination)?;
        tracing::info!(
            socket = %compositor.socket_name.to_string_lossy(),
            "accepting clients; start them with WAYLAND_DISPLAY set to the socket"
        );
        log_process_usage("at start");
        while compositor.is_running {
            event_loop
                .dispatch(None, &mut compositor)
                .map_err(RunError::WaitForEvents)?;
            display
                .dispatch_clients(&mut compositor)
                .map_err(RunError::DispatchClients)?;
            compositor.popups.cleanup();
            if let Err(error) = display.flush_clients() {
                tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    "could not send to clients"
                );
            }
        }
        tracing::info!("stopping");
        tracing::debug!(resources = %compositor.ledger, "resources at exit");
        log_process_usage("at exit");
        Ok(())
    }

    /// Sets up every subsystem and starts accepting clients.
    fn new(
        handle: &LoopHandle<'static, Self>,
        display: &Display<Self>,
        options: RunOptions,
        termination: TerminationSignals,
    ) -> Result<Self, RunError> {
        let display_handle = display.handle();
        let ledger = ResourceLedger::new();
        let mut protocols = ProtocolStates::new(&display_handle);
        let seat = Self::create_seat(&mut protocols.seat, &display_handle)?;
        let backend = Backend::start(options.backend, handle, &display_handle, &ledger)?;

        let socket = ListeningSocketSource::new_auto().map_err(RunError::Listen)?;
        let socket_name = socket.socket_name().to_os_string();
        let listening = EventSource::insert(handle, &ledger, socket, |stream, (), compositor| {
            compositor.accept_client(stream);
        })
        .map_err(|error| RunError::RegisterSource(error.error))?;
        // Readiness only wakes the loop; `run` dispatches the clients.
        let display_descriptor = display
            .as_fd()
            .try_clone_to_owned()
            .map_err(RunError::DuplicateDisplayDescriptor)?;
        let display_wakeup = EventSource::insert(
            handle,
            &ledger,
            Generic::new(display_descriptor, Interest::READ, Mode::Level),
            |_, _, _| Ok(PostAction::Continue),
        )
        .map_err(|error| RunError::RegisterSource(error.error))?;
        // A termination request stops vaht cleanly: clients are told, and
        // the exit resource report is written.
        let termination = EventSource::insert(
            handle,
            &ledger,
            termination.into_source(),
            |event, (), compositor| {
                tracing::info!(signal = ?event.signal(), "asked to stop");
                compositor.stop();
            },
        )
        .map_err(|error| RunError::RegisterSource(error.error))?;

        Ok(Self {
            display_handle,
            ledger,
            protocols,
            seat,
            backend,
            windows: Windows::default(),
            popups: PopupManager::default(),
            theme: Theme::built_in(),
            capture: ScreenCapture::new().map_err(RunError::CaptureRenderer)?,
            typesetter: Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH),
            held_titlebar: None,
            focused: None,
            start_time: Instant::now(),
            is_running: true,
            socket_name,
            _event_sources: [listening, display_wakeup, termination],
        })
    }

    /// Asks vaht to stop after the current events.
    pub(crate) fn stop(&mut self) {
        self.is_running = false;
    }

    /// Asks for a frame to be drawn soon.
    pub(crate) fn schedule_render(&mut self) {
        self.backend.schedule_render();
    }

    /// Draws a frame, then tells the windows in it that they were shown.
    pub(crate) fn render(&mut self) {
        let metrics = self.theme.metrics();
        for window in self.windows.iter_mut() {
            let geometry = window.frame_geometry(metrics);
            window
                .frame_mut()
                .prepare(&geometry, &self.theme, &mut self.typesetter);
        }
        if let Err(error) = self.backend.render(&self.windows, metrics) {
            tracing::warn!(error = &error as &dyn std::error::Error, "frame skipped");
        }
        let time = self.start_time.elapsed();
        for window in self.windows.iter() {
            window.toplevel().send_frame(self.backend.output(), time);
        }
    }

    /// Starts managing a new application window, on top and focused.
    pub(crate) fn add_window(&mut self, surface: ToplevelSurface) {
        let location = self.cascade_location();
        let id = self
            .windows
            .insert(Toplevel::from_xdg(surface), location, &self.ledger);
        tracing::debug!(window = %id, "window opened");
        self.focus_window(Some(id), smithay::utils::SERIAL_COUNTER.next_serial());
    }

    /// Stops managing the window whose root surface is `surface`.
    pub(crate) fn remove_window(&mut self, surface: &WlSurface) {
        let Some(id) = self.windows.id_of(surface) else {
            return;
        };
        let window = self.windows.remove(id);
        tracing::debug!(
            window = %id,
            app_id = ?window.as_ref().and_then(|window| window.toplevel().app_id()),
            "window closed"
        );
        if self.focused == Some(id) {
            self.focused = None;
            self.focus_window(
                self.windows.top(),
                smithay::utils::SERIAL_COUNTER.next_serial(),
            );
        }
        self.schedule_render();
    }

    /// Moves the window with `id` above all others.
    pub(crate) fn raise_window(&mut self, id: WindowId) {
        if self.windows.raise(id) {
            self.schedule_render();
        }
    }

    /// Gives keyboard focus to the window with `id`, or to nothing.
    pub(crate) fn focus_window(&mut self, id: Option<WindowId>, serial: Serial) {
        if self.focused != id {
            if let Some(previous) = self
                .focused
                .and_then(|previous| self.windows.get_mut(previous))
            {
                previous.toplevel().set_activated(false);
                previous.frame_mut().set_focused(false);
            }
            if let Some(window) = id.and_then(|id| self.windows.get_mut(id)) {
                window.toplevel().set_activated(true);
                window.frame_mut().set_focused(true);
                tracing::debug!(window = %window.id(), title = ?window.toplevel().title(), "focused");
            }
            self.focused = id;
        }
        let surface = id
            .and_then(|id| self.windows.get(id))
            .map(|window| window.toplevel().wl_surface().clone());
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        // Smithay clears the focus of a destroyed surface without telling
        // SeatHandler::focus_changed, so the clipboard focus is set here too:
        // otherwise it keeps the last focused client's state alive after
        // that client disconnects.
        let client = surface
            .as_ref()
            .and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, &self.seat, client);
        keyboard.set_focus(self, surface, serial);
        self.schedule_render();
    }

    /// Updates what is known of a surface after its client commits it.
    pub(crate) fn handle_commit(&mut self, surface: &WlSurface) {
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self
                .windows
                .id_of(&root)
                .and_then(|id| self.windows.get_mut(id))
            {
                window.toplevel().on_commit();
                if &root == surface {
                    window.toplevel().configure_if_unconfigured();
                    window.sync_with_client();
                }
            }
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
            && let Err(error) = popup.send_configure()
        {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not configure a popup"
            );
        }
        self.schedule_render();
    }

    /// Accepts a client that connected to the socket.
    fn accept_client(&self, stream: std::os::unix::net::UnixStream) {
        let state = Arc::new(ClientState::new(&self.ledger));
        if let Err(error) = self.display_handle.clone().insert_client(stream, state) {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not accept a client"
            );
        }
    }

    /// Returns where the next window's content goes: each window further
    /// down and right than the last, starting over every few windows, with
    /// room above for its frame.
    fn cascade_location(&self) -> Point<i32, Logical> {
        let step = i32::try_from(self.windows.len() % Self::CASCADE_LENGTH)
            .expect("the cascade is far shorter than i32::MAX steps");
        let offset = Self::CASCADE_STEP * (step + 1);
        let frame_top =
            FrameGeometry::around(Rectangle::default(), self.theme.metrics()).borders[0].loc;
        self.backend.output_geometry().loc + Point::from((offset, offset)) - frame_top
    }

    /// Creates the seat with its keyboard and pointer.
    fn create_seat(
        state: &mut SeatState<Self>,
        display: &DisplayHandle,
    ) -> Result<Seat<Self>, RunError> {
        let mut seat = state.new_wl_seat(display, "seat0");
        seat.add_keyboard(
            XkbConfig::default(),
            Self::KEY_REPEAT_DELAY_MILLISECONDS,
            Self::KEY_REPEAT_RATE,
        )
        .map_err(RunError::Keyboard)?;
        seat.add_pointer();
        Ok(seat)
    }
}

impl fmt::Debug for Compositor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Compositor")
            .field("socket_name", &self.socket_name)
            .field("windows", &self.windows.len())
            .field("focused", &self.focused)
            .finish_non_exhaustive()
    }
}

/// How vaht should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunOptions {
    /// Which backend to run on.
    pub backend: BackendChoice,
}

/// Logs the process's open descriptors and memory, `moment` saying when.
///
/// The counts at start and at exit match when nothing leaked; the tests
/// compare them.
fn log_process_usage(moment: &str) {
    match ProcessUsage::read() {
        Ok(usage) => tracing::debug!(
            descriptors = usage.descriptors.total(),
            resident_bytes = ?usage.resident_bytes,
            census = %usage.descriptors,
            "process usage {moment}"
        ),
        Err(error) => tracing::warn!(
            error = &error as &dyn std::error::Error,
            "could not read process usage {moment}"
        ),
    }
}

/// Why vaht could not start or keep running.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The event loop could not be created.
    #[error("could not create the event loop")]
    EventLoop(#[source] calloop::Error),

    /// The Wayland display could not be created.
    #[error("could not create the Wayland display")]
    Display(#[source] smithay::reexports::wayland_server::backend::InitError),

    /// No socket for clients could be opened.
    #[error("could not open a socket for clients")]
    Listen(#[source] smithay::reexports::wayland_server::BindError),

    /// The display's descriptor could not be duplicated for the event loop.
    #[error("could not watch the Wayland display for client activity")]
    DuplicateDisplayDescriptor(#[source] io::Error),

    /// The keyboard's keymap could not be compiled.
    #[error("could not set up the keyboard")]
    Keyboard(#[source] smithay::input::keyboard::Error),

    /// The software renderer that draws screenshots could not be created.
    #[error("could not create the screenshot renderer")]
    CaptureRenderer(#[source] smithay::backend::renderer::pixman::PixmanError),

    /// No backend could start.
    #[error("could not start a backend")]
    Backend(#[from] BackendError),

    /// A source could not be registered with the event loop.
    #[error("could not register with the event loop")]
    RegisterSource(#[source] calloop::Error),

    /// Waiting for events failed.
    #[error("could not wait for events")]
    WaitForEvents(#[source] calloop::Error),

    /// Reading from clients failed.
    #[error("could not read from clients")]
    DispatchClients(#[source] io::Error),
}

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
use std::num::NonZeroU32;
use std::os::fd::AsFd as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::desktop::{PopupKind, PopupManager};
use smithay::input::keyboard::{KeymapFile, XkbConfig};
use smithay::input::{Seat, SeatState};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle, Resource as _};
use smithay::utils::{Rectangle, Serial};
use smithay::wayland::compositor::{get_parent, is_sync_subsurface};
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::set_primary_focus;
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::socket::ListeningSocketSource;
use vaht_ui::{Theme, Typesetter};

use crate::backend::{Backend, BackendChoice, BackendError};
use crate::capture::ScreenCapture;
use crate::client::ClientState;
use crate::config::{Config, ConfigWatch};
use crate::event_source::{EventSource, Timer};
use crate::focus::Focus;
use crate::input::InputState;
use crate::launcher::Launcher;
use crate::layers::{Layers, OutputLayers};
use crate::placement_strategy::Room;
use crate::protocols::ProtocolStates;
use crate::resources::{self, ResourceLedger};
use crate::scene::Scene;
use crate::signals::{ProcessSignals, SignalRequest};
use crate::window::{Frame, NewWindow, OutputArea, Toplevel, Window, WindowId};
use crate::windows::Windows;
use crate::workspaces::{Workspace, Workspaces};

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
    /// vaht's own keymap, restored after a virtual keyboard used its own.
    pub(crate) physical_keymap: KeymapFile,
    /// Whether a client released a surface or a capture session since the
    /// last cleanup: renderers and Smithay may still reference its memory.
    pub(crate) client_cleanup_pending: bool,
    /// The window whose titlebar the pointer is holding down, if any.
    pub(crate) held_titlebar: Option<WindowId>,
    /// The configuration in effect.
    pub(crate) config: Config,
    /// Where the configuration files are.
    config_directory: PathBuf,
    /// Watches the configuration directory, while it exists.
    config_watch: Option<ConfigWatch>,
    /// Whether the directory was removed or renamed, so the watch sees
    /// nothing any more and must be renewed.
    config_watch_is_stale: bool,
    /// Reloads the configuration once a burst of file changes settles.
    reload_timer: Option<Timer<Self>>,
    /// Starts programs and reaps them.
    pub(crate) launcher: Launcher,
    /// Key and button state between events: repeats, double clicks.
    pub(crate) input: InputState,
    /// Registers timers and other sources with the event loop.
    pub(crate) loop_handle: LoopHandle<'static, Self>,
    /// Which window has keyboard focus, and which had it before.
    pub(crate) focus: Focus,
    /// The window a snap step down just minimized: a step up restores it
    /// while the modifiers that went down for it are held.
    pub(crate) snap_minimized: Option<WindowId>,
    /// The workspaces, and which one is shown.
    pub(crate) workspaces: Workspaces,
    /// The mapped layer surfaces.
    pub(crate) layers: Layers,
    /// When vaht started; frame times are measured from it.
    pub(crate) start_time: Instant,
    /// False once vaht has been asked to stop.
    is_running: bool,
    /// The name of the socket clients connect to (`WAYLAND_DISPLAY`).
    socket_name: OsString,
    /// The listening socket, the display's wake-up descriptor, and the
    /// signals vaht handles.
    _event_sources: [EventSource<Self>; 3],
}

impl Compositor {
    /// How long a held key waits before repeating; repeating key bindings
    /// use it too.
    pub(crate) const KEY_REPEAT_DELAY: Duration = Duration::from_millis(200);

    /// How often a held key repeats, per second; repeating key bindings
    /// use it too, unless they set their own rate.
    pub(crate) const KEY_REPEAT_RATE: NonZeroU32 =
        NonZeroU32::new(25).expect("the key repeat rate is not zero");

    /// How long the configuration files must be left alone after a change
    /// before they are read: an editor's save is several changes.
    const RELOAD_DELAY: Duration = Duration::from_millis(200);

    /// Runs vaht until it is asked to stop: by its host window closing, by
    /// a termination signal among `signals`, or by the `exit` action.
    ///
    /// # Errors
    ///
    /// Returns an error if vaht cannot start, or if waiting for events fails.
    pub fn run(options: RunOptions, signals: ProcessSignals) -> Result<(), RunError> {
        let mut event_loop: EventLoop<'static, Self> =
            EventLoop::try_new().map_err(RunError::EventLoop)?;
        let mut display: Display<Self> = Display::new().map_err(RunError::Display)?;
        let mut compositor = Self::new(&event_loop.handle(), &display, options, signals)?;
        tracing::info!(
            socket = %compositor.socket_name.to_string_lossy(),
            "accepting clients; start them with WAYLAND_DISPLAY set to the socket"
        );
        resources::log_process_usage("at start");
        while compositor.is_running {
            event_loop
                .dispatch(None, &mut compositor)
                .map_err(RunError::WaitForEvents)?;
            display
                .dispatch_clients(&mut compositor)
                .map_err(RunError::DispatchClients)?;
            compositor.popups.cleanup();
            compositor.release_dead_client_resources();
            if let Err(error) = display.flush_clients() {
                tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    "could not send to clients"
                );
            }
        }
        tracing::info!("stopping");
        tracing::debug!(resources = %compositor.ledger, "resources at exit");
        resources::log_process_usage("at exit");
        Ok(())
    }

    /// Sets up every subsystem and starts accepting clients.
    fn new(
        handle: &LoopHandle<'static, Self>,
        display: &Display<Self>,
        options: RunOptions,
        signals: ProcessSignals,
    ) -> Result<Self, RunError> {
        let display_handle = display.handle();
        let ledger = ResourceLedger::new();
        let mut protocols = ProtocolStates::new(&display_handle);
        let seat = Self::create_seat(&mut protocols.seat, &display_handle)?;
        let physical_keymap = seat
            .get_keyboard()
            .expect("create_seat adds a keyboard")
            .keymap_file();
        let mut backend = Backend::start(options.backend, handle, &display_handle, &ledger)?;
        if let Some(formats) = backend.dmabuf_formats() {
            protocols.dmabuf_global = Some(
                protocols
                    .dmabuf
                    .create_global::<Self>(&display_handle, formats),
            );
        }

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
        let signals = EventSource::insert(
            handle,
            &ledger,
            signals.into_source(),
            |event, (), compositor| match SignalRequest::of(event.signal()) {
                Some(SignalRequest::Stop) => {
                    tracing::info!(signal = ?event.signal(), "asked to stop");
                    compositor.stop();
                }
                Some(SignalRequest::Reload) => compositor.reload_configuration(),
                Some(SignalRequest::ReapPrograms) => compositor.launcher.reap(),
                None => {}
            },
        )
        .map_err(|error| RunError::RegisterSource(error.error))?;
        let config = Config::load(&options.config_directory).unwrap_or_else(|error| {
            tracing::error!(
                error = &error as &dyn std::error::Error,
                "the configuration is not valid; using the built-in one until it is fixed"
            );
            Config::built_in()
        });

        let launcher = Launcher::new(socket_name.clone(), &ledger);
        let workspaces = Workspaces::named(config.settings.workspaces.names());
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
            config,
            config_watch: None,
            config_watch_is_stale: false,
            config_directory: options.config_directory,
            reload_timer: None,
            launcher,
            input: InputState::default(),
            loop_handle: handle.clone(),
            client_cleanup_pending: false,
            physical_keymap,
            focus: Focus::default(),
            snap_minimized: None,
            workspaces,
            layers: Layers::default(),
            start_time: Instant::now(),
            is_running: true,
            socket_name,
            _event_sources: [listening, display_wakeup, signals],
        })
        .map(|mut compositor| {
            compositor.watch_configuration();
            for command_line in compositor.config.settings.session.autostart.clone() {
                compositor.launcher.run(&command_line);
            }
            compositor
        })
    }

    /// Reads the configuration again and applies it if it is valid;
    /// otherwise the current one stays, and the problem is logged.
    pub(crate) fn reload_configuration(&mut self) {
        self.watch_configuration();
        match Config::load(&self.config_directory) {
            Ok(config) => {
                self.config = config;
                self.apply_settings();
                tracing::info!(directory = %self.config_directory.display(), "configuration reloaded");
            }
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                "configuration rejected; the previous one stays in effect"
            ),
        }
    }

    /// Brings what follows the settings up to date with them: the
    /// workspaces' names and number, and the titlebars' buttons.
    fn apply_settings(&mut self) {
        let settings = &self.config.settings;
        if let Some(last) = self.workspaces.rename(settings.workspaces.names()) {
            for window in self.windows.iter_mut() {
                if window.workspace() > last {
                    window.move_to_workspace(last);
                }
            }
        }
        let buttons = settings.windows.frame.titlebar_layout();
        for window in self.windows.iter_mut() {
            window.frame_mut().set_buttons(&buttons);
        }
        self.focus_most_recent();
        self.schedule_render();
    }

    /// Notes that a configuration file changed, or with `directory_gone`
    /// that the directory itself went away, and reloads once things settle.
    pub(crate) fn configuration_changed(&mut self, directory_gone: bool) {
        self.config_watch_is_stale |= directory_gone;
        match Timer::once(
            &self.loop_handle,
            &self.ledger,
            Self::RELOAD_DELAY,
            Self::reload_configuration,
        ) {
            // Replacing a pending timer cancels it: the burst goes on.
            Ok(timer) => self.reload_timer = Some(timer),
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not schedule a configuration reload"
            ),
        }
    }

    /// Watches the configuration directory, unless it already is or the
    /// directory does not exist; a reload tries again.
    fn watch_configuration(&mut self) {
        if self.config_watch.is_some() && !self.config_watch_is_stale {
            return;
        }
        match ConfigWatch::start(&self.loop_handle, &self.ledger, &self.config_directory) {
            Ok(watch) => {
                self.config_watch = Some(watch);
                self.config_watch_is_stale = false;
                tracing::debug!(directory = %self.config_directory.display(), "watching the configuration");
            }
            Err(error) => {
                tracing::debug!(
                    error = &error as &dyn std::error::Error,
                    directory = %self.config_directory.display(),
                    "not watching the configuration; it reloads on SIGHUP or the reload action"
                );
            }
        }
    }

    /// Returns the window with keyboard focus.
    pub(crate) const fn focused(&self) -> Option<WindowId> {
        self.focus.focused()
    }

    /// Returns what the output shows now.
    pub(crate) fn scene(&self) -> Scene<'_> {
        Scene::new(
            &self.windows,
            self.theme.metrics(),
            self.workspaces.current(),
            self.output_layers(),
        )
    }

    /// Returns the output's layer surfaces.
    pub(crate) fn output_layers(&self) -> OutputLayers<'_> {
        OutputLayers::new(self.backend.output(), self.backend.output_geometry())
    }

    /// Returns the parts of the output windows can be placed in: all of
    /// it, and what panels leave free.
    pub(crate) fn output_area(&self) -> OutputArea {
        OutputArea {
            whole: self.backend.output_geometry(),
            usable: self.output_layers().usable_area(),
        }
    }

    /// Gives the keyboard to a layer surface that wants it to itself, or
    /// back to the focused window when none does any more.
    pub(crate) fn refresh_layer_keyboard(&mut self) {
        let exclusive = self.output_layers().exclusive_keyboard_surface();
        if self.layers.set_keyboard(exclusive) {
            self.focus_window(self.focused(), smithay::utils::SERIAL_COUNTER.next_serial());
        }
    }

    /// Gives the keyboard to the layer surface `surface`, clicked, if it
    /// takes the keyboard on a click; returns whether it did.
    pub(crate) fn focus_layer(&mut self, surface: &WlSurface) -> bool {
        if !self.layers.takes_keyboard_on_click(surface) {
            return false;
        }
        self.layers.set_keyboard(Some(surface.clone()));
        self.focus_window(self.focused(), smithay::utils::SERIAL_COUNTER.next_serial());
        true
    }

    /// Lets go of everything still referencing what clients released, if
    /// anything was released since the last call.
    ///
    /// Smithay keeps a list of capture sessions and renderers cache client
    /// buffers; both are pruned only when asked. Without this, a closed
    /// capture session keeps its client's memory pool mapped forever, and
    /// the list grows with every session (found by the screenshot leak test).
    fn release_dead_client_resources(&mut self) {
        if std::mem::take(&mut self.client_cleanup_pending) {
            self.protocols.image_copy_capture.cleanup();
            self.backend.release_unused_textures();
            self.capture.release_unused_textures();
            // The last references to a gone client's buffers go with the next
            // frame, so make sure there is one.
            self.schedule_render();
        }
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
        self.prepare_panels();
        let output = self.backend.output().clone();
        let scene = Scene::new(
            &self.windows,
            self.theme.metrics(),
            self.workspaces.current(),
            OutputLayers::new(&output, self.backend.output_geometry()),
        );
        if let Err(error) = self.backend.render(&scene) {
            tracing::warn!(error = &error as &dyn std::error::Error, "frame skipped");
        }
        // Hidden windows get no frame callbacks, so they stop drawing.
        let time = self.start_time.elapsed();
        for window in scene.windows() {
            window.toplevel().send_frame(&output, time);
        }
        for layer in self.layers.iter() {
            layer.send_frame(&output, time, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
        tracing::trace!("frame drawn");
    }

    /// Brings every panel's pixels up to date with what it shows; drawing a
    /// frame and taking a screenshot both start here, so a screenshot shows
    /// the same as the screen would.
    pub(crate) fn prepare_panels(&mut self) {
        let metrics = self.theme.metrics();
        for window in self.windows.iter_mut() {
            let geometry = window.frame_geometry(metrics);
            window
                .frame_mut()
                .prepare(&geometry, &self.theme, &mut self.typesetter);
        }
    }

    /// Starts managing a new application window, on top, and focused if
    /// the settings say new windows take focus. It is placed once its
    /// client gives it a size.
    pub(crate) fn add_window(&mut self, surface: ToplevelSurface) {
        let buttons = self.config.settings.windows.frame.titlebar_layout();
        let new = NewWindow {
            toplevel: Toplevel::from_xdg(surface),
            frame: Frame::new(buttons, &self.ledger),
            workspace: self.workspaces.current(),
        };
        let id = self.windows.insert(new, &self.ledger);
        tracing::debug!(window = %id, "window opened");
        if self.config.settings.focus.focus_new_windows {
            self.focus_window(Some(id), smithay::utils::SERIAL_COUNTER.next_serial());
        }
    }

    /// Gives the window `id` its first place, by the placement settings,
    /// among the windows already shown.
    fn place_new_window(&mut self, id: WindowId) {
        let metrics = self.theme.metrics();
        let current = self.workspaces.current();
        let occupied = self
            .windows
            .shown_top_down(current)
            .filter(|window| window.id() != id && window.is_placed())
            .map(|window| window.outer_area(metrics))
            .collect();
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup")
            .current_location()
            .to_i32_round();
        let room = Room {
            usable: self.output_area().usable,
            occupied,
            pointer,
        };
        let output = self.output_area();
        let placement = self.config.settings.placement;
        let Some(window) = self.windows.get_mut(id) else {
            return;
        };
        let rule = self.config.rules.for_window(window, current);
        let size = rule.size().unwrap_or(window.outer_area(metrics).size);
        let corner = rule.position.map_or_else(
            || {
                placement.strategy.place(
                    size,
                    &room,
                    placement.row_direction,
                    placement.column_direction,
                )
            },
            |position| position.corner(room.usable, size),
        );
        let outer = Rectangle::new(corner, size);
        window.place_initially(outer, metrics);
        if let Some(workspace) = rule.workspace.map(Workspace::numbered)
            && workspace.is_within(&self.workspaces)
        {
            window.move_to_workspace(workspace);
        }
        if let Some(is_sticky) = rule.sticky {
            window.set_sticky(is_sticky);
        }
        if let Some(is_minimized) = rule.minimized {
            window.set_minimized(is_minimized);
        }
        if let Some(placement) = rule.placement {
            window.place(placement.placement(outer), output, metrics);
        }
        // A rule may have sent the new window out of sight.
        self.focus_most_recent();
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
        let was_focused = self.focused() == Some(id);
        self.focus.forget(id);
        if was_focused {
            self.focus_most_recent();
        }
        self.schedule_render();
    }

    /// Moves the window with `id` below all others.
    pub(crate) fn lower_window(&mut self, id: WindowId) {
        if self.windows.lower(id) {
            self.schedule_render();
        }
    }

    /// Moves the window with `id` above all others.
    pub(crate) fn raise_window(&mut self, id: WindowId) {
        if self.windows.raise(id) {
            self.schedule_render();
        }
    }

    /// Gives focus to the window that had it most recently among those
    /// shown, if the focused one is not shown: it was minimized, closed,
    /// or left on another workspace.
    pub(crate) fn focus_most_recent(&mut self) {
        let current = self.workspaces.current();
        let is_shown = |id| {
            self.windows
                .get(id)
                .is_some_and(|window| window.is_shown_on(current))
        };
        if self.focused().is_some_and(is_shown) {
            return;
        }
        let shown = self
            .windows
            .shown_top_down(current)
            .map(Window::id)
            .collect::<Vec<_>>();
        let most_recent = self.focus.by_recency(shown).first().copied();
        self.focus_window(most_recent, smithay::utils::SERIAL_COUNTER.next_serial());
    }

    /// Gives keyboard focus to the window with `id`, or to nothing.
    pub(crate) fn focus_window(&mut self, id: Option<WindowId>, serial: Serial) {
        let previous_focus = self.focused();
        if previous_focus != id {
            if let Some(previous) =
                previous_focus.and_then(|previous| self.windows.get_mut(previous))
            {
                previous.toplevel().set_activated(false);
                previous.frame_mut().set_focused(false);
            }
            if let Some(window) = id.and_then(|id| self.windows.get_mut(id)) {
                window.toplevel().set_activated(true);
                window.frame_mut().set_focused(true);
                tracing::debug!(window = %window.id(), title = ?window.toplevel().title(), "focused");
            }
        }
        self.focus.set(id);
        let surface = id
            .and_then(|id| self.windows.get(id))
            .map(|window| window.toplevel().wl_surface().clone());
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        // A layer surface holding the keyboard (a launcher) keeps it.
        let surface = self.layers.keyboard().cloned().or(surface);
        // Smithay clears the focus of a destroyed surface without telling
        // SeatHandler::focus_changed, so the clipboard focus is set here too:
        // otherwise it keeps the last focused client's state alive after
        // that client disconnects.
        let client = surface
            .as_ref()
            .and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, &self.seat, client.clone());
        set_primary_focus(&self.display_handle, &self.seat, client);
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
                    if window.is_ready_for_placement() {
                        let id = window.id();
                        self.place_new_window(id);
                    }
                }
            }
        }
        if self.output_layers().commit(surface) {
            self.refresh_layer_keyboard();
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

    /// Creates the seat with its keyboard and pointer.
    fn create_seat(
        state: &mut SeatState<Self>,
        display: &DisplayHandle,
    ) -> Result<Seat<Self>, RunError> {
        let mut seat = state.new_wl_seat(display, "seat0");
        seat.add_keyboard(
            XkbConfig::default(),
            i32::try_from(Self::KEY_REPEAT_DELAY.as_millis()).expect("the delay is short"),
            i32::try_from(Self::KEY_REPEAT_RATE.get()).expect("the rate is small"),
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
            .field("focused", &self.focused())
            .finish_non_exhaustive()
    }
}

/// How vaht should run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    /// Which backend to run on.
    pub backend: BackendChoice,
    /// Where the configuration files are (normally `~/.config/vaht`).
    pub config_directory: PathBuf,
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

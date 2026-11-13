//! The compositor's state, and the loop that runs it.
//!
//! [`Compositor`] is the state Smithay's handler traits are implemented on.
//! It owns the subsystems and routes events between them; it decides
//! nothing about windows. The window manager ([`WindowManager`]) decides,
//! and the compositor carries its decisions out through Smithay:
//! - each event loop round ends by *settling* ([`Compositor::run`]): what
//!   the window manager asked of clients is sent, keyboard focus follows its
//!   choice, and a frame is drawn if anything shown changed;
//! - before an input event reaches a client, the same settling happens, so
//!   a key typed right after a binding moved focus goes where focus went.
//!
//! Its methods are the ones that need the whole state: Smithay's seat calls
//! (`set_focus`, pointer motion and buttons, grabs) take `&mut Compositor`
//! and call back into its handler traits, so they cannot run inside a
//! subsystem's method (DESIGN.md, "Where behavior lives").
//!
//! The Wayland [`Display`] is deliberately *not* part of that state. Clients
//! are dispatched with `display.dispatch_clients(&mut compositor)`, which
//! needs both mutably at once; keeping them apart makes that an ordinary
//! borrow, where Smithay's examples reach the display through `unsafe`
//! (R-X3). The event loop only watches a duplicate of the display's
//! descriptor to wake up, and clients are dispatched after every wake-up.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent, InputTime, KeyState,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::desktop::{
    PopupKind, PopupManager, find_popup_root_surface, get_popup_toplevel_coords,
};
use smithay::input::keyboard::{
    FilterResult, KeyboardSource, Keycode, KeymapFile, KeysymHandle, ModifiersState, XkbConfig,
};
use smithay::input::pointer::{ButtonEvent, Focus, GrabStartData, MotionEvent};
use smithay::input::{Seat, SeatState};
use smithay::reexports::calloop::{EventLoop, LoopHandle};
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle, Resource as _, Weak};
use smithay::utils::{Logical, Point, SERIAL_COUNTER, Serial};
use smithay::wayland::compositor::{get_parent, is_sync_subsurface};
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::set_primary_focus;
use smithay::wayland::shell::xdg::{PopupSurface, ToplevelSurface};
use smithay::wayland::virtual_keyboard::VirtualKeyboardKeyEvent;
use vaht_ui::Typesetter;

use crate::backend::{Backend, BackendChoice, BackendError};
use crate::capture::ScreenCapture;
use crate::client::{ClientListener, ListenError};
use crate::config::{
    Action, Command, Config, ConfigSource, FocusModel, KeyChord, Modifiers, ResizeFrom,
};
use crate::event_source::EventSource;
use crate::execution::{ExecutionContext, Press};
use crate::grab::{GrabKind, WindowGrab};
use crate::input::{self, AfterClick, ButtonPress, Input, PressOutcome};
use crate::launcher::Launcher;
use crate::layers::{Layers, OutputLayers};
use crate::protocols::ProtocolStates;
use crate::resources::{self, ResourceKind, ResourceLedger};
use crate::scene::{FrameTextures, PointerTarget, Scene, WindowTarget};
use crate::signals::ProcessSignals;
use crate::toplevels::{Toplevel, Toplevels};
use crate::window_manager::{
    ClientRequest, FramePart, OutputArea, Window, WindowDrag, WindowId, WindowManager,
};

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
    /// Windows, workspaces and focus, and every rule about them.
    pub(crate) window_manager: WindowManager,
    /// The client's side of every window.
    pub(crate) toplevels: Toplevels,
    /// Every window's frame, as the renderer takes it.
    pub(crate) frame_textures: FrameTextures,
    /// Popups, by the surface they belong to.
    pub(crate) popups: PopupManager,
    /// The mapped layer surfaces.
    pub(crate) layers: Layers,
    /// Shapes and draws text; shared by every panel.
    typesetter: Typesetter,
    /// Draws screenshots for capture clients.
    pub(crate) capture: ScreenCapture,
    /// vaht's own keymap, restored after a virtual keyboard used its own.
    physical_keymap: KeymapFile,
    /// The surface keyboard focus was last given to: the seat's focus as
    /// vaht set it. Held weakly, so it never keeps a gone client's surface,
    /// and the buffers it holds, alive.
    keyboard_focus: Option<Weak<WlSurface>>,
    /// Key and button state between events: repeats, double clicks.
    input: Input,
    /// Whether a client released a surface or a capture session since the
    /// last cleanup: renderers and Smithay may still reference its memory.
    pub(crate) client_cleanup_pending: bool,
    /// How many clients were connected at the last cleanup; fewer now means
    /// some left, and what they held must be let go.
    connected_clients: usize,
    /// The configuration in effect.
    pub(crate) config: Config,
    /// Where the configuration comes from, and when it is read again.
    pub(crate) config_source: ConfigSource,
    /// Starts programs and reaps them.
    pub(crate) launcher: Launcher,
    /// Registers timers and other sources with the event loop.
    pub(crate) loop_handle: LoopHandle<'static, Self>,
    /// When vaht started; frame times are measured from it.
    pub(crate) start_time: Instant,
    /// False once vaht has been asked to stop.
    is_running: bool,
    /// The socket clients connect to.
    listener: ClientListener,
    /// The signals vaht handles, registered with the event loop.
    _signals: EventSource<Self>,
}

impl Compositor {
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
            socket = %compositor.listener.socket_name().to_string_lossy(),
            "accepting clients; start them with WAYLAND_DISPLAY set to the socket"
        );
        resources::log_process_usage("at start");
        while compositor.is_running {
            event_loop
                .dispatch(None, &mut compositor)
                .map_err(RunError::WaitForEvents)?;
            compositor.settle();
            display
                .dispatch_clients(&mut compositor)
                .map_err(RunError::DispatchClients)?;
            compositor.popups.cleanup();
            // Settled first, so nothing vaht holds for a gone client stays
            // referenced while its resources are released.
            compositor.settle();
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

        let listener = ClientListener::open(handle, display, &ledger)?;
        let signals = signals
            .watch(handle, &ledger)
            .map_err(RunError::WatchSignals)?;
        let config_source = ConfigSource::new(options.config_directory);
        let config = config_source.load().unwrap_or_else(|error| {
            tracing::error!(
                error = &error as &dyn std::error::Error,
                "the configuration is not valid; using the built-in one until it is fixed"
            );
            Config::built_in()
        });

        let whole = backend.output_geometry();
        let window_manager = WindowManager::new(
            config.settings.workspaces.names(),
            OutputArea {
                whole,
                usable: whole,
            },
            config.theme.metrics(),
        );
        let launcher = Launcher::new(listener.socket_name().to_os_string(), &ledger);
        let mut compositor = Self {
            display_handle,
            ledger,
            protocols,
            seat,
            backend,
            window_manager,
            toplevels: Toplevels::default(),
            frame_textures: FrameTextures::default(),
            popups: PopupManager::default(),
            layers: Layers::default(),
            typesetter: Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH),
            capture: ScreenCapture::new().map_err(RunError::CaptureRenderer)?,
            physical_keymap,
            keyboard_focus: None,
            input: Input::default(),
            client_cleanup_pending: false,
            connected_clients: 0,
            config,
            config_source,
            launcher,
            loop_handle: handle.clone(),
            start_time: Instant::now(),
            is_running: true,
            listener,
            _signals: signals,
        };
        compositor
            .config_source
            .watch(&compositor.loop_handle, &compositor.ledger);
        for command_line in &compositor.config.settings.session.autostart {
            compositor.launcher.run(command_line);
        }
        Ok(compositor)
    }

    /// Returns what the output shows now.
    pub(crate) fn scene(&self) -> Scene<'_> {
        Scene::new(
            &self.window_manager,
            &self.toplevels,
            &self.frame_textures,
            self.output_layers(),
        )
    }

    /// Returns the output's layer surfaces.
    pub(crate) fn output_layers(&self) -> OutputLayers<'_> {
        OutputLayers::new(self.backend.output(), self.backend.output_geometry())
    }

    /// Asks vaht to stop after the current events.
    pub(crate) const fn stop(&mut self) {
        self.is_running = false;
    }

    /// Reads the configuration again and applies it if it is valid;
    /// otherwise the current one stays, and the problem is logged.
    pub(crate) fn reload_configuration(&mut self) {
        self.config_source.watch(&self.loop_handle, &self.ledger);
        match self.config_source.load() {
            Ok(config) => {
                self.window_manager.apply_settings(&config.settings);
                if config.theme != self.config.theme {
                    self.window_manager.restyle(config.theme.metrics());
                }
                self.config = config;
                tracing::info!(
                    directory = %self.config_source.directory().display(),
                    "configuration reloaded"
                );
            }
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                "configuration rejected; the previous one stays in effect"
            ),
        }
    }

    /// Asks for a frame to be drawn soon.
    pub(crate) fn schedule_render(&mut self) {
        self.backend.schedule_render();
    }

    /// Draws a frame, then tells the windows and layer surfaces in it that
    /// they were shown; hidden windows get no frame callbacks, so they stop
    /// drawing.
    pub(crate) fn render(&mut self) {
        self.prepare_frames();
        let output = self.backend.output().clone();
        let scene = Scene::new(
            &self.window_manager,
            &self.toplevels,
            &self.frame_textures,
            OutputLayers::new(&output, self.backend.output_geometry()),
        );
        if let Err(error) = self.backend.render(&scene) {
            tracing::warn!(error = &error as &dyn std::error::Error, "frame skipped");
        }
        let time = self.start_time.elapsed();
        for (_, toplevel) in scene.windows() {
            toplevel.send_frame(&output, time);
        }
        for layer in self.layers.iter() {
            layer.send_frame(&output, time, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
        tracing::trace!("frame drawn");
    }

    /// Brings every frame's pixels up to date with what it shows; drawing a
    /// frame and taking a screenshot both start here, so a screenshot shows
    /// the same as the screen would.
    pub(crate) fn prepare_frames(&mut self) {
        self.window_manager
            .paint_frames(&self.config.theme, &mut self.typesetter);
        self.frame_textures
            .update(&self.window_manager, &self.config.theme, &self.ledger);
    }

    /// Starts managing the window of a new xdg-shell toplevel.
    pub(crate) fn add_window(&mut self, surface: ToplevelSurface) {
        let window = self
            .window_manager
            .add_window(&self.config.settings, &self.ledger);
        self.toplevels
            .insert(window, Toplevel::from_xdg(surface), &self.ledger);
    }

    /// Stops managing the window whose root surface is `surface`.
    pub(crate) fn remove_window(&mut self, surface: &WlSurface) {
        if let Some(window) = self.toplevels.remove(surface) {
            self.window_manager.remove_window(window);
        }
    }

    /// Updates what is known of a surface after its client commits it.
    pub(crate) fn handle_commit(&mut self, surface: &WlSurface) {
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            self.commit_window(&root, surface == &root);
        }
        if self.output_layers().commit(surface) {
            self.refresh_layer_keyboard();
            self.refresh_output_area();
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

    /// Updates layer surfaces after `surface`, one of them, is gone.
    pub(crate) fn remove_layer_surface(&mut self, surface: &WlSurface) {
        let output = self.backend.output().clone();
        self.layers.unmap(surface, &output);
        self.refresh_layer_keyboard();
        self.refresh_output_area();
        self.schedule_render();
    }

    /// Delivers one input event.
    pub(crate) fn handle_input<B: InputBackend>(&mut self, event: InputEvent<B>) {
        // A key typed right after a binding moved focus goes where it went.
        self.settle();
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "Smithay's input enum has many kinds vaht does not handle yet"
        )]
        match event {
            InputEvent::Keyboard { event } => {
                self.activate_keymap(None);
                self.press_key(
                    KeyboardSource::Physical,
                    event.key_code(),
                    event.state(),
                    event.time(),
                );
            }
            InputEvent::PointerMotion { event } => {
                let output = self.backend.output_geometry().to_f64();
                let moved = self.pointer_location() + event.delta();
                // The far edges are outside the output, so stop just short.
                let location = Point::from((
                    moved
                        .x
                        .clamp(output.loc.x, output.loc.x + output.size.w - 1.0),
                    moved
                        .y
                        .clamp(output.loc.y, output.loc.y + output.size.h - 1.0),
                ));
                self.move_pointer_to(location, event.time());
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let output = self.backend.output_geometry();
                let location = event.position_transformed(output.size) + output.loc.to_f64();
                self.move_pointer_to(location, event.time());
            }
            InputEvent::PointerButton { event } => self.handle_pointer_button::<B>(&event),
            InputEvent::PointerAxis { event } => self.handle_pointer_axis::<B>(&event),
            _ => {}
        }
    }

    /// Sends a key from a virtual keyboard, read with that keyboard's own
    /// keymap (or the seat's, when it sent none).
    pub(crate) fn handle_virtual_key(&mut self, event: &VirtualKeyboardKeyEvent) {
        self.settle();
        let device = event.device();
        self.activate_keymap(device.keymap());
        self.press_key(
            device.source(),
            event.key_code(),
            event.state(),
            event.time(),
        );
    }

    /// Releases every key a virtual keyboard still holds; it is going away.
    pub(crate) fn release_virtual_keys(&mut self, source: KeyboardSource) {
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        keyboard.release_source(self, source);
    }

    /// Sets the modifier state a virtual keyboard asks for: `masks` are its
    /// depressed, latched and locked modifiers and its layout group, relative
    /// to its `keymap` (or the seat's).
    pub(crate) fn set_virtual_modifiers(&mut self, keymap: Option<KeymapFile>, masks: [u32; 4]) {
        self.activate_keymap(keymap);
        let [depressed, latched, locked, group] = masks;
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        keyboard.with_xkb_state(self, |mut context| {
            context.set_modifier_mask(depressed, latched, locked, group);
        });
        if depressed == 0 {
            self.window_manager.modifiers_released();
        }
    }

    /// Runs `command` in `context`.
    pub(crate) fn execute(&mut self, command: &Command, context: &mut ExecutionContext<'_>) {
        match command {
            Command::Run(action) => self.run_action(action, context),
            Command::Sequence(commands) => {
                for command in commands {
                    self.execute(command, context);
                }
            }
            // The key handling repeats it; here it runs once.
            Command::Repeat { command, .. } => self.execute(command, context),
        }
    }

    /// Starts `drag` on `window` with the pointer button held since `start`.
    pub(crate) fn start_window_grab(
        &mut self,
        window: WindowId,
        drag: WindowDrag,
        start: GrabStartData<Self>,
        serial: Serial,
    ) {
        if self.window_manager.begin_drag(window, drag, start.location) {
            self.set_grab(WindowGrab::drag(window, drag, start, serial));
        }
    }

    /// Passes a client's `request` for the window of `surface` on to the
    /// window manager; the client hears back even when nothing changes, as
    /// xdg-shell requires.
    pub(crate) fn honor_client_request(
        &mut self,
        surface: &ToplevelSurface,
        request: ClientRequest,
    ) {
        let Some(window) = self.toplevels.window_of(surface.wl_surface()) else {
            return;
        };
        if !self.window_manager.honor(window, request)
            && let Some(toplevel) = self.toplevels.get(window)
        {
            toplevel.acknowledge();
        }
    }

    /// Starts `drag` on the window of `surface`, as its client asks, if the
    /// request is genuine: the pointer button behind `serial` is still
    /// held, on a surface of the same client. Anything else is a stale or
    /// forged request, and is ignored.
    pub(crate) fn start_requested_drag(
        &mut self,
        surface: &ToplevelSurface,
        seat: &WlSeat,
        serial: Serial,
        drag: WindowDrag,
    ) {
        let Some(pointer) = Seat::<Self>::from_resource(seat).and_then(|seat| seat.get_pointer())
        else {
            return;
        };
        let Some(start) = pointer
            .grab_start_data()
            .filter(|_| pointer.has_grab(serial))
        else {
            return;
        };
        let is_same_client = start
            .focus
            .as_ref()
            .is_some_and(|(focus, _)| focus.id().same_client_as(&surface.wl_surface().id()));
        if let Some(window) = self.toplevels.window_of(surface.wl_surface())
            && is_same_client
        {
            self.start_window_grab(window, drag, start, serial);
        }
    }

    /// Keeps `popup` within its window's output, flipping or sliding it as
    /// its positioner allows.
    pub(crate) fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Some(location) = find_popup_root_surface(&kind)
            .ok()
            .and_then(|root| self.toplevels.window_of(&root))
            .and_then(|window| self.window_manager.window(window))
            .map(Window::location)
        else {
            return;
        };
        // The positioner works relative to the popup's parent.
        let mut target = self.backend.output_geometry();
        target.loc -= get_popup_toplevel_coords(&kind);
        target.loc -= location;
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    /// Returns the parts of the output windows can be placed in: all of
    /// it, and what panels leave free.
    fn output_area(&self) -> OutputArea {
        OutputArea {
            whole: self.backend.output_geometry(),
            usable: self.output_layers().usable_area(),
        }
    }

    /// Returns where the pointer is.
    fn pointer_location(&self) -> Point<f64, Logical> {
        self.seat
            .get_pointer()
            .expect("the seat gets a pointer at startup")
            .current_location()
    }

    /// Returns the modifiers held now.
    fn held_modifiers(&self) -> Modifiers {
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        Modifiers::held(&keyboard.modifier_state())
    }

    /// Carries out what the window manager decided since the last call:
    /// places windows can be put, what clients are asked, where keyboard
    /// focus goes, and whether a frame is needed.
    fn settle(&mut self) {
        self.refresh_output_area();
        let updates = self.window_manager.take_client_updates();
        for (window, requested) in &updates.states {
            if let Some(toplevel) = self.toplevels.get(*window) {
                toplevel.request(requested);
            }
        }
        for (window, ending) in updates.endings {
            if let Some(toplevel) = self.toplevels.get(window) {
                toplevel.end(ending, &self.display_handle);
            }
        }
        self.sync_keyboard_focus();
        if self.window_manager.take_redraw() {
            self.schedule_render();
        }
    }

    /// Gives the keyboard to a layer surface that wants it to itself, or
    /// back to the windows when none does any more.
    fn refresh_layer_keyboard(&mut self) {
        let output = OutputLayers::new(self.backend.output(), self.backend.output_geometry());
        self.layers.refresh_keyboard(output);
    }

    /// Tells the window manager where windows can be placed now.
    fn refresh_output_area(&mut self) {
        let output = self.output_area();
        self.window_manager.set_output_area(output);
    }

    /// Gives the seat's keyboard to the focused window, unless a layer
    /// surface holds it (a launcher keeps it), if that changed.
    fn sync_keyboard_focus(&mut self) {
        let window_surface = self
            .window_manager
            .focused()
            .and_then(|window| self.toplevels.get(window))
            .map(|toplevel| toplevel.wl_surface().clone());
        let surface = self.layers.keyboard().cloned().or(window_surface);
        // A focused surface that is gone reads as `Some(None)`, unlike any
        // surface or none, so its client's focus is always cleared.
        let given = self
            .keyboard_focus
            .as_ref()
            .map(|given| given.upgrade().ok());
        if given == surface.clone().map(Some) {
            return;
        }
        self.keyboard_focus = surface.as_ref().map(WlSurface::downgrade);
        // Smithay clears the focus of a destroyed surface without telling
        // SeatHandler::focus_changed, so the clipboard focus is set here too:
        // otherwise it keeps the last focused client's state alive after
        // that client disconnects.
        let client = surface
            .as_ref()
            .and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, &self.seat, client.clone());
        set_primary_focus(&self.display_handle, &self.seat, client);
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
    }

    /// Updates the window whose root surface is `root` after a commit of
    /// it (`is_root`) or of one of its subsurfaces.
    fn commit_window(&mut self, root: &WlSurface, is_root: bool) {
        let Some(window) = self.toplevels.window_of(root) else {
            return;
        };
        let Some(toplevel) = self.toplevels.get(window) else {
            return;
        };
        toplevel.on_commit();
        if !is_root {
            return;
        }
        let committed = toplevel.committed_state();
        let pointer = self.pointer_location().to_i32_round();
        self.window_manager
            .commit(window, committed, &self.config, pointer);
        // The first configure carries what the window manager asked.
        self.settle();
        if let Some(toplevel) = self.toplevels.get(window) {
            toplevel.configure_if_unconfigured();
        }
    }

    /// Lets go of everything still referencing what clients released, if
    /// anything was released, or a client left, since the last call.
    ///
    /// Smithay keeps a list of capture sessions and renderers cache client
    /// buffers; both are pruned only when asked. Without this, a closed
    /// capture session keeps its client's memory pool mapped forever, and
    /// the list grows with every session (found by the screenshot leak test).
    fn release_dead_client_resources(&mut self) {
        let connected = self.ledger.count(ResourceKind::Client).live;
        let has_client_left = connected < self.connected_clients;
        self.connected_clients = connected;
        if std::mem::take(&mut self.client_cleanup_pending) || has_client_left {
            self.protocols.image_copy_capture.cleanup();
            self.backend.release_unused_textures();
            self.capture.release_unused_textures();
            // The last references to a gone client's buffers go with the next
            // frame, so make sure there is one.
            self.schedule_render();
        }
    }

    /// Runs one action.
    fn run_action(&mut self, action: &Action, context: &mut ExecutionContext<'_>) {
        tracing::debug!(%action, window = ?context.window, "running an action");
        match action {
            Action::WindowManager(action) => self.window_manager.run(action, context.window),
            Action::Exec(command_line) => self.launcher.run(command_line),
            Action::StartMoving => self.start_drag(None, context),
            Action::StartResizing(from) => self.start_drag(Some(*from), context),
            Action::Reload => self.reload_configuration(),
            Action::Exit => self.stop(),
            Action::SetEnvironment { name, value } => {
                self.launcher.set_environment(name.clone(), value.clone());
            }
        }
    }

    /// Starts moving the context's window with the pointer, or resizing it
    /// when `resize` says from where, or turns the drag in progress into
    /// that.
    fn start_drag(&mut self, resize: Option<ResizeFrom>, context: &mut ExecutionContext<'_>) {
        let (Some(window), Some(press)) = (context.window, context.press.as_ref()) else {
            tracing::warn!(
                "start-moving and start-resizing work only in mouse bindings on a window"
            );
            return;
        };
        let metrics = self.window_manager.metrics();
        let Some(target) = self.window_manager.window(window) else {
            return;
        };
        let pointer = press.start.location;
        let drag = match resize {
            None => WindowDrag::Move,
            Some(from) => WindowDrag::Resize {
                edges: from.edges(target.frame_geometry(metrics).outer, pointer),
            },
        };
        if let Some(grab) = context.grab.as_deref_mut() {
            if *grab == GrabKind::Hold && self.window_manager.begin_drag(window, drag, pointer) {
                *grab = GrabKind::Drag(drag);
            }
        } else {
            let (start, serial) = (press.start.clone(), press.serial);
            self.start_window_grab(window, drag, start, serial);
        }
    }

    /// Gives the pointer to `grab`.
    fn set_grab(&mut self, grab: WindowGrab) {
        let serial = grab.serial();
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// Makes `keymap` the seat's keymap, or vaht's own for `None`, if it is
    /// not already: a virtual keyboard's keycodes mean something only in its
    /// own keymap.
    fn activate_keymap(&mut self, keymap: Option<KeymapFile>) {
        let wanted = keymap.unwrap_or_else(|| self.physical_keymap.clone());
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        if keyboard.keymap_file().id() != wanted.id()
            && let Err(error) = keyboard.set_keymap_from_keymap_file(self, wanted)
        {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not switch keymaps"
            );
        }
    }

    /// Sends a key from `source` to the focused window, unless it is bound.
    fn press_key(
        &mut self,
        source: KeyboardSource,
        keycode: Keycode,
        state: KeyState,
        time: InputTime,
    ) {
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        let bound = keyboard.input_from_source(
            source,
            self,
            keycode,
            state,
            SERIAL_COUNTER.next_serial(),
            time,
            |compositor, modifiers, keysym| {
                compositor.filter_key(keycode, state, modifiers, &keysym)
            },
        );
        if let Some(Some(command)) = bound {
            self.run_key_binding(keycode, command);
        }
    }

    /// Decides whether a key event is vaht's: a bound press, or the release
    /// of one. Returns the bound press's command.
    fn filter_key(
        &mut self,
        keycode: Keycode,
        state: KeyState,
        modifiers: &ModifiersState,
        keysym: &KeysymHandle<'_>,
    ) -> FilterResult<Option<Command>> {
        let held = Modifiers::held(modifiers);
        if state == KeyState::Released && held == Modifiers::default() {
            self.window_manager.modifiers_released();
        }
        let chord = keysym
            .raw_latin_sym_or_raw_current_sym()
            .map(|key| KeyChord::new(held, key));
        self.input.key(keycode, state, chord, &self.config.bindings)
    }

    /// Runs a bound key's `command` for the focused window, and starts
    /// repeating it while `keycode` is held if it repeats.
    fn run_key_binding(&mut self, keycode: Keycode, command: Command) {
        let focused = self.window_manager.focused();
        self.execute(&command, &mut ExecutionContext::for_window(focused));
        if let Command::Repeat { rate, .. } = &command {
            let rate = *rate;
            self.input.repeat_key_binding(
                keycode,
                command,
                rate,
                (&self.loop_handle, &self.ledger),
            );
        }
    }

    /// Moves the pointer to `location`, telling the surface under it.
    fn move_pointer_to(&mut self, location: Point<f64, Logical>, time: InputTime) {
        let target = self.scene().target_at(location);
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        if let Some(window) = target.as_ref().and_then(PointerTarget::window)
            && !pointer.is_grabbed()
        {
            self.pointer_entered(window);
        }
        // Over a frame, no client has the pointer: the frame is vaht's.
        let under = target.as_ref().and_then(PointerTarget::client_surface);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }

    /// With sloppy focus, focuses `window`, which the pointer is over, and
    /// with auto-raise raises it once the pointer has rested there a while.
    fn pointer_entered(&mut self, window: WindowId) {
        let focus = self.config.settings.focus;
        if focus.model != FocusModel::Sloppy || self.window_manager.focused() == Some(window) {
            return;
        }
        self.window_manager.focus(Some(window));
        self.input.cancel_raise();
        if focus.auto_raise {
            self.input.raise_later(
                window,
                focus.auto_raise_delay(),
                (&self.loop_handle, &self.ledger),
            );
        }
    }

    /// Presses or releases a pointer button.
    fn handle_pointer_button<B: InputBackend>(&mut self, event: &B::PointerButtonEvent) {
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let serial = SERIAL_COUNTER.next_serial();
        let location = pointer.current_location();
        match event.state() {
            ButtonState::Pressed if !pointer.is_grabbed() => {
                self.press_button(event.button_code(), location, serial);
            }
            ButtonState::Pressed => {}
            ButtonState::Released => self.window_manager.release_titlebar_button(location),
        }
        // A client clicked has keyboard focus by the time it hears of it.
        self.settle();
        pointer.button(
            self,
            &ButtonEvent {
                button: event.button_code(),
                state: event.state(),
                serial,
                time: event.time(),
            },
        );
        pointer.frame(self);
    }

    /// Handles a press of `button` at `location` while nothing holds the
    /// pointer, as `Input` decides: focuses and raises the window there,
    /// then holds a titlebar button, runs a binding, or resizes from a
    /// border.
    fn press_button(&mut self, button: u32, location: Point<f64, Logical>, serial: Serial) {
        self.window_manager.finish_cycle();
        let target = self.scene().target_at(location);
        let is_on_titlebar_button = match &target {
            Some(PointerTarget::Window(WindowTarget::Frame {
                window,
                part: FramePart::Titlebar(_),
            })) => self
                .window_manager
                .has_titlebar_button_at(*window, location),
            _ => false,
        };
        let press = ButtonPress {
            button,
            target: target.as_ref(),
            modifiers: self.held_modifiers(),
            is_on_titlebar_button,
            time: Instant::now(),
        };
        let (window, then) = match self.input.press(&press, &self.config) {
            PressOutcome::ClickLayer(surface) => {
                self.layers.click(&surface);
                return;
            }
            PressOutcome::Click {
                window,
                raises,
                then,
            } => {
                if let Some(raised) = window.filter(|_| raises) {
                    self.window_manager.raise(raised);
                }
                self.window_manager.focus(window);
                (window, then)
            }
        };
        let start = Press {
            start: GrabStartData {
                focus: None,
                button,
                location,
            },
            serial,
        };
        match then {
            AfterClick::HoldTitlebarButton(pressed) => {
                self.window_manager.press_titlebar_button(pressed, location);
            }
            AfterClick::RunBinding { press, drag } => {
                if let Some(command) = press {
                    let mut context = ExecutionContext::for_press(window, start.clone());
                    self.execute(&command, &mut context);
                }
                let pointer = self
                    .seat
                    .get_pointer()
                    .expect("the seat gets a pointer at startup");
                // Unless the binding started a drag, hold the press, so it
                // never reaches a client.
                if !pointer.is_grabbed() {
                    self.set_grab(WindowGrab::hold(window, drag.map(|drag| *drag), start));
                }
            }
            AfterClick::ResizeBorder(resized, edges) => {
                self.start_window_grab(resized, WindowDrag::Resize { edges }, start.start, serial);
            }
            AfterClick::PassToClient => {}
        }
    }

    /// Scrolls the surface under the pointer, unless the wheel is bound
    /// there.
    fn handle_pointer_axis<B: InputBackend>(&mut self, event: &B::PointerAxisEvent) {
        if self.run_scroll_binding(event.amount_v120(Axis::Vertical)) {
            return;
        }
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        pointer.axis(self, input::axis_frame::<B>(event));
        pointer.frame(self);
    }

    /// Runs what a wheel turn of `steps` (in 120ths of a notch, positive
    /// down) is bound to where the pointer is; returns whether it was bound.
    fn run_scroll_binding(&mut self, steps: Option<f64>) -> bool {
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let Some(steps) = steps.filter(|_| !pointer.is_grabbed()) else {
            return false;
        };
        let target = self.scene().target_at(pointer.current_location());
        let Some(command) = self
            .config
            .bindings
            .scroll(target.as_ref(), self.held_modifiers(), steps)
            .cloned()
        else {
            return false;
        };
        let window = target.as_ref().and_then(PointerTarget::window);
        self.execute(&command, &mut ExecutionContext::for_window(window));
        true
    }

    /// Creates the seat with its keyboard and pointer.
    fn create_seat(
        state: &mut SeatState<Self>,
        display: &DisplayHandle,
    ) -> Result<Seat<Self>, RunError> {
        let mut seat = state.new_wl_seat(display, "seat0");
        seat.add_keyboard(
            XkbConfig::default(),
            i32::try_from(Input::KEY_REPEAT_DELAY.as_millis()).expect("the delay is short"),
            i32::try_from(Input::KEY_REPEAT_RATE.get()).expect("the rate is small"),
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
            .field("socket_name", &self.listener.socket_name())
            .field("windows", &self.window_manager.window_count())
            .field("focused", &self.window_manager.focused())
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

    /// vaht could not listen for clients.
    #[error("could not listen for clients")]
    Listen(#[from] ListenError),

    /// The signals vaht handles could not be registered.
    #[error("could not watch for signals")]
    WatchSignals(#[source] calloop::Error),

    /// The keyboard's keymap could not be compiled.
    #[error("could not set up the keyboard")]
    Keyboard(#[source] smithay::input::keyboard::Error),

    /// The software renderer that draws screenshots could not be created.
    #[error("could not create the screenshot renderer")]
    CaptureRenderer(#[source] smithay::backend::renderer::pixman::PixmanError),

    /// No backend could start.
    #[error("could not start a backend")]
    Backend(#[from] BackendError),

    /// Waiting for events failed.
    #[error("could not wait for events")]
    WaitForEvents(#[source] calloop::Error),

    /// Reading from clients failed.
    #[error("could not read from clients")]
    DispatchClients(#[source] io::Error),
}

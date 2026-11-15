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
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use jiff::Zoned;
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent, InputTime, KeyState,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent, Switch,
    SwitchState, SwitchToggleEvent as _,
};
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::desktop::{
    PopupKind, PopupManager, find_popup_root_surface, get_popup_toplevel_coords,
};
use smithay::input::keyboard::{
    FilterResult, KeyboardSource, Keycode, KeymapFile, KeysymHandle, ModifiersState, XkbConfig,
};
use smithay::input::pointer::{
    ButtonEvent, Focus, GrabStartData, MotionEvent, RelativeMotionEvent,
};
use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::{EventLoop, LoopHandle};
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle, Resource as _};
use smithay::utils::{IsAlive as _, Logical, Point, Rectangle, SERIAL_COUNTER, Serial};
use smithay::wayland::compositor::{get_parent, is_sync_subsurface};
use smithay::wayland::dmabuf::DmabufGlobal;
use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat as _;
use smithay::wayland::pointer_constraints::{PointerConstraint, with_pointer_constraint};
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::set_primary_focus;
use smithay::wayland::shell::xdg::{PopupSurface, ToplevelSurface};
use smithay::wayland::virtual_keyboard::VirtualKeyboardKeyEvent;
use vaht_ui::Typesetter;

use crate::backend::{
    Backend, BackendChoice, BackendError, OutputEvent, TopWindow, virtual_terminal,
};
use crate::buffer_fences::FenceWaits;
use crate::capture::ScreenCapture;
use crate::client::{ClientListener, ListenError};
use crate::config::{
    Action, Command, Config, ConfigSource, Delay, FocusModel, KeyChord, MenuGenerator, MenuRequest,
    Menus, Modifiers, ResizeFrom, SettingChange, SettingOverrides, ThemeFile, WindowManagerAction,
};
use crate::config::{
    FocusStealing, MenuEntry, OutputEntry, OutputMode, OutputScale, OutputTransform, SwitchFlip,
    SwitchKind, TaskbarAction, TaskbarOutput,
};
use crate::cursor::Cursor;
use crate::event_source::{EndingSource, EventSource, Timer};
use crate::execution::{ExecutionContext, Press};
use crate::focus_target::FocusTarget;
use crate::frame_pacing::FramePacing;
use crate::gestures;
use crate::grab::{GrabKind, WindowGrab};
use crate::idle::IdleInhibitors;
use crate::input::{self, AfterClick, ButtonPress, Input, KeyPress, PressOutcome};
use crate::input_emulation::{DeviceSetup, Emulated, InputEmulation};
use crate::ipc::IpcServer;
use crate::launcher::Launcher;
use crate::layers::{LayerMaps, Layers, OutputLayers};
use crate::menu::{
    MenuBuilder, MenuChoice, MenuItem, MenuKey, MenuModel, MenuStack, OutputChoices, PipeRead,
    parse_printed,
};
use crate::output_management::{HeadRequest, HeadState};
use crate::output_memory::OutputMemory;
use crate::outputs::{OutputChanges, Outputs};
use crate::pointer_lock::{self, PointerHold};
use crate::protocols::{OutputControls, ProtocolStates};
use crate::resources::{self, ResourceKind, ResourceLedger};
use crate::scene::{
    Backdrop, Drawn, FrameTextures, Outline, PointerTarget, Scene, SnapOverlay, WindowTarget,
};
use crate::session_environment;
use crate::session_lock::SessionLock;
use crate::signals::ProcessSignals;
use crate::tablet;
use crate::taskbar::TaskbarHit;
use crate::taskbars::Taskbars;
use crate::tooltip::Tooltip;
use crate::toplevel_list::{ForeignToplevels, ToplevelList};
use crate::toplevel_management::{
    Covering, HandleRequest, ToplevelManagement, ToplevelStates, WindowFacts,
};
use crate::toplevels::{Toplevel, Toplevels};
use crate::touch;
use crate::tray::TrayAction;
use crate::window_manager::{
    Axes, ClientRequest, FramePart, OutputArea, OutputAreas, Placement, PlacementKind, Window,
    WindowDrag, WindowId, WindowManager, WorkspaceTarget,
};
use crate::workspace_pager::{WorkspaceFacts, WorkspaceRequest};
use crate::xwayland::XwaylandServer;
use smithay::wayland::foreign_toplevel_list::ForeignToplevelHandle;
use smithay::xwayland::{X11Surface, X11Wm};

/// Where the autostart programs are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Autostart {
    /// Held until Xwayland is ready, so that X programs among them find it
    /// in `DISPLAY`.
    WaitingForXwayland,
    /// Started, or never held.
    Started,
}

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
    /// The outputs the backend shows on, and where each is.
    pub(crate) outputs: Outputs,
    /// The pointer's picture, for outputs vaht draws the pointer on.
    pub(crate) cursor: Cursor,
    /// Xwayland, for X11 programs, unless the settings turn it off or it
    /// could not start.
    pub(crate) xwayland: Option<XwaylandServer>,
    /// Where a game holding the pointer locked last said it is, on its
    /// surface: where the pointer goes when the lock ends.
    pub(crate) pointer_lock_hint: Option<(WlSurface, Point<f64, Logical>)>,
    /// The surfaces asking that the session not go idle.
    pub(crate) idle_inhibitors: IdleInhibitors<WlSurface>,
    /// The session lock.
    pub(crate) session_lock: SessionLock,
    /// Holds back and lets through clients' paced and timed frames.
    pub(crate) frame_pacing: FramePacing,
    /// Holds clients' frames until the GPU has drawn them.
    pub(crate) fence_waits: FenceWaits,
    /// Whether the activation environment is told of the session.
    exports_activation_environment: bool,
    /// Where the autostart programs are.
    autostart: Autostart,
    /// Delayed commands waiting to run.
    delays: Vec<(Delay, Timer<Self>)>,
    /// Where windows were on outputs that left, to go back to them.
    output_memory: OutputMemory<WindowId>,
    /// The EIS server X programs emulate input through, with Xwayland.
    input_emulation: Option<InputEmulation>,
    /// The windows as programs outside vaht see them listed.
    pub(crate) toplevel_list: ToplevelList<WindowId, ForeignToplevelHandle>,
    /// The windows as `zwlr_foreign_toplevel_manager_v1` tells them.
    pub(crate) toplevel_management: ToplevelManagement,
    /// What programs outside vaht do to the outputs: arrange them, turn
    /// them off and change their colors.
    pub(crate) output_controls: OutputControls,
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
    /// The taskbar, if the settings want one.
    pub(crate) taskbars: Taskbars,
    /// The menus open.
    pub(crate) menus: MenuStack,
    /// The wallpaper.
    pub(crate) backdrop: Backdrop,
    /// The outline of an outline resize.
    pub(crate) outline: Outline,
    /// Where a moved window would snap, and the layout picker.
    pub(crate) snap_overlay: SnapOverlay,
    /// The program of the tray item last clicked, and when: it may bring
    /// its windows forward for `activation_grace_ms`.
    pub(crate) tray_grant: Option<(u32, Instant)>,
    /// The tooltip for what the pointer rests on.
    pub(crate) tooltip: Tooltip,
    /// Shapes and draws text; shared by every panel.
    typesetter: Typesetter,
    /// Finds application icons, for the taskbar and menus.
    icons: vaht_ui::IconStore,
    /// Draws screenshots for capture clients.
    pub(crate) capture: ScreenCapture,
    /// vaht's own keymap, restored after a virtual keyboard used its own.
    physical_keymap: KeymapFile,
    /// The surface keyboard focus was last given to: the seat's focus as
    /// vaht set it. Held weakly, so it never keeps a gone client's surface,
    /// and the buffers it holds, alive.
    pub(crate) keyboard_focus: Option<FocusTarget>,
    /// Key and button state between events: repeats, double clicks.
    pub(crate) input: Input,
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
    /// Settings changed in place, not yet saved.
    overrides: SettingOverrides,
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
    /// The sockets sandboxed programs connect to, one per sandbox; dropped
    /// once ended.
    pub(crate) sandbox_listeners: Vec<EndingSource<Self>>,
    /// The IPC socket, unless it could not be opened.
    pub(crate) ipc: Option<IpcServer>,
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
            if let Some(ipc) = &mut compositor.ipc {
                ipc.release_ended();
            }
            compositor.launcher.release_ended_outputs();
            compositor.follow_input_emulation_question();
            compositor.settle();
            display
                .dispatch_clients(&mut compositor)
                .map_err(RunError::DispatchClients)?;
            compositor.popups.cleanup();
            // Settled first, so nothing vaht holds for a gone client stays
            // referenced while its resources are released.
            compositor.settle();
            compositor.release_dead_client_resources();
            // Last, so what clients changed just now is in it.
            compositor.publish_ipc_events();
            compositor.announce_outputs();
            compositor.announce_workspaces();
            if let Err(error) = display.flush_clients() {
                tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    "could not send to clients"
                );
            }
        }
        compositor.update_activation_environment(true);
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
        let mut protocols = ProtocolStates::new(&display_handle, handle);
        let seat = Self::create_seat(&mut protocols.seat, &display_handle)?;
        let physical_keymap = seat
            .get_keyboard()
            .expect("create_seat adds a keyboard")
            .keymap_file();
        let allows_xwayland = options.allows_xwayland;
        let mut backend = Backend::start(options.backend, handle, &display_handle, &ledger)?;
        protocols.dmabuf_global =
            Self::dmabuf_global(&mut protocols, &mut backend, &display_handle);

        let listener = ClientListener::open(handle, display, &ledger)?;
        let signals = signals
            .watch(handle, &ledger)
            .map_err(RunError::WatchSignals)?;
        let config_source = ConfigSource::new(options.config_directory);
        let config = Self::first_config(&config_source);

        let mut outputs = backend.outputs();
        let mut output_changes = outputs.configure(&config.outputs, &mut backend);
        // At start, the primary output is news to the hooks whatever it is.
        output_changes.primary = Some(outputs.primary().name());
        let window_manager = WindowManager::new(
            &config.settings,
            Self::bare_output_areas(&outputs),
            config.theme.metrics(),
        );
        let mut launcher = Launcher::new(
            listener.socket_name().to_os_string(),
            options.executable,
            handle,
            &ledger,
        );
        let ipc = Self::open_ipc(handle, &listener, &ledger);
        if let Some(server) = &ipc {
            launcher.set_environment(
                vaht_ipc::SOCKET_VARIABLE.to_owned(),
                server.path().to_string_lossy().into_owned(),
            );
        }
        let mut compositor = Self {
            display_handle,
            ledger,
            protocols,
            seat,
            backend,
            outputs,
            cursor: Cursor::from_environment(),
            xwayland: None,
            pointer_lock_hint: None,
            idle_inhibitors: IdleInhibitors::default(),
            session_lock: SessionLock::default(),
            frame_pacing: FramePacing::default(),
            fence_waits: FenceWaits::default(),
            exports_activation_environment: false,
            autostart: Autostart::Started,
            delays: Vec::new(),
            output_memory: OutputMemory::default(),
            input_emulation: None,
            toplevel_list: ToplevelList::default(),
            toplevel_management: ToplevelManagement::new(&display.handle()),
            output_controls: OutputControls::new(&display.handle()),
            window_manager,
            toplevels: Toplevels::default(),
            frame_textures: FrameTextures::default(),
            popups: PopupManager::default(),
            layers: Layers::default(),
            taskbars: Taskbars::new(&config.settings.taskbar),
            menus: MenuStack::with_search(config.settings.menus.search),
            backdrop: Backdrop::new(&config.settings.wallpaper, config_source.directory()),
            outline: Outline::default(),
            snap_overlay: SnapOverlay::default(),
            tooltip: Tooltip::default(),
            tray_grant: None,
            typesetter: Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH),
            icons: vaht_ui::IconStore::from_environment(),
            capture: ScreenCapture::default(),
            physical_keymap,
            keyboard_focus: None,
            input: Input::default(),
            client_cleanup_pending: false,
            connected_clients: 0,
            config,
            config_source,
            overrides: SettingOverrides::default(),
            launcher,
            loop_handle: handle.clone(),
            start_time: Instant::now(),
            is_running: true,
            listener,
            sandbox_listeners: Vec::new(),
            ipc,
            _signals: signals,
        };
        compositor.start(allows_xwayland, &output_changes);
        Ok(compositor)
    }

    /// Starts what runs alongside the compositor once it is made: the
    /// configuration's watch, the taskbar's services, input devices'
    /// settings, Xwayland (if `allows_xwayland`), the autostart programs
    /// (once Xwayland is ready, if it is starting), and the outputs'
    /// `output_changes`.
    fn start(&mut self, allows_xwayland: bool, output_changes: &OutputChanges) {
        self.config_source.watch(&self.loop_handle, &self.ledger);
        self.start_taskbar_services();
        if let Backend::Drm(backend) = &mut self.backend {
            backend.set_input_settings(&self.config.settings.input);
        }
        self.start_xwayland(allows_xwayland);
        let owns_seat = matches!(self.backend, Backend::Drm(_));
        self.exports_activation_environment = self
            .config
            .settings
            .session
            .update_activation_environment
            .applies(owns_seat);
        if self.xwayland.is_some() {
            self.autostart = Autostart::WaitingForXwayland;
            // Wayland services find the session meanwhile.
            self.update_activation_environment(false);
        } else {
            self.run_autostart();
        }
        self.follow_output_changes(output_changes);
    }

    /// Tells the activation environment of the session, then runs the
    /// autostart programs, which may need the services it starts.
    fn run_autostart(&mut self) {
        self.autostart = Autostart::Started;
        let autostart = &self.config.settings.session.autostart;
        if let Some(command) = self.activation_update(false) {
            self.launcher.run_in_order(&command, autostart);
        } else {
            for command_line in autostart {
                self.launcher.run(command_line);
            }
        }
    }

    /// Runs the autostart programs held for Xwayland, if they still wait;
    /// else tells the activation environment what changed.
    fn finish_xwayland_start(&mut self) {
        if self.autostart == Autostart::WaitingForXwayland {
            self.run_autostart();
        } else {
            self.update_activation_environment(false);
        }
    }

    /// Hears that Xwayland stopped before it was ready: the autostart
    /// programs run without it.
    pub(crate) fn xwayland_failed(&mut self) {
        tracing::warn!("Xwayland stopped before it was ready; autostart runs without X");
        self.finish_xwayland_start();
    }

    /// Offers `linux-dmabuf` with what `backend` reads: with feedback on
    /// the hardware, where surfaces are told which buffers suit them; none
    /// where the backend takes no GPU buffers.
    fn dmabuf_global(
        protocols: &mut ProtocolStates,
        backend: &mut Backend,
        display: &DisplayHandle,
    ) -> Option<DmabufGlobal> {
        if let Some(feedback) = backend.dmabuf_default_feedback() {
            return Some(
                protocols
                    .dmabuf
                    .create_global_with_default_feedback::<Self>(display, &feedback),
            );
        }
        let formats = backend.dmabuf_formats()?;
        Some(protocols.dmabuf.create_global::<Self>(display, formats))
    }

    /// Returns the configuration in `source`, or the built-in one, with an
    /// error logged, if it is not valid.
    fn first_config(source: &ConfigSource) -> Config {
        source
            .load(&SettingOverrides::default())
            .unwrap_or_else(|error| {
                tracing::error!(
                    error = &error as &dyn std::error::Error,
                    "the configuration is not valid; using the built-in one until it is fixed"
                );
                Config::built_in()
            })
    }

    /// Opens the IPC socket beside the Wayland one `listener` opened; none,
    /// with a warning, if it cannot be: vaht works without it.
    fn open_ipc(
        handle: &LoopHandle<'static, Self>,
        listener: &ClientListener,
        ledger: &ResourceLedger,
    ) -> Option<IpcServer> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
        let path = vaht_ipc::socket_path(Path::new(&runtime), listener.socket_name());
        match IpcServer::open(path.clone(), handle, ledger) {
            Ok(server) => {
                tracing::info!(socket = %path.display(), "accepting IPC clients");
                Some(server)
            }
            Err(error) => {
                tracing::warn!(%error, socket = %path.display(), "no IPC: could not open its socket");
                None
            }
        }
    }

    /// Returns what the output shows now.
    pub(crate) fn scene(&self) -> Scene<'_> {
        Scene::new(
            &self.window_manager,
            &self.toplevels,
            Drawn {
                frames: &self.frame_textures,
                taskbars: &self.taskbars,
                menus: &self.menus,
                backdrop: &self.backdrop,
                outline: &self.outline,
                tooltip: &self.tooltip,
                snap: &self.snap_overlay,
                xwayland: self.xwayland.as_ref(),
                lock: &self.session_lock,
            },
            self.layer_maps(),
        )
    }

    /// Returns every output's layer surfaces.
    pub(crate) fn layer_maps(&self) -> LayerMaps<'_> {
        LayerMaps::new(&self.outputs)
    }

    /// Asks vaht to stop after the current events.
    pub(crate) const fn stop(&mut self) {
        self.is_running = false;
    }

    /// Reads the configuration again and applies it if it is valid;
    /// otherwise the current one stays, and the problem is logged.
    pub(crate) fn reload_configuration(&mut self) {
        self.config_source.watch(&self.loop_handle, &self.ledger);
        if let Ok(file) = self.config_source.settings_table() {
            self.overrides.follow_file(&file);
        }
        let loaded = match self.config_source.load(&self.overrides) {
            Err(error) if !self.overrides.is_empty() => {
                let without = self.config_source.load(&SettingOverrides::default());
                if without.is_ok() {
                    tracing::warn!(
                        error = &error as &dyn std::error::Error,
                        "the settings changed in place no longer apply; they are dropped"
                    );
                    self.overrides = SettingOverrides::default();
                }
                without
            }
            loaded => loaded,
        };
        match loaded {
            Ok(config) => {
                self.apply_config(config);
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

    /// Makes `change` to the settings in place, until `save-settings`
    /// writes it, if the configuration stays valid with it.
    fn change_setting(&mut self, change: SettingChange) {
        let changed = self.config_source.settings_table().and_then(|file| {
            let overrides = self.overrides.with(change, &file);
            let config = self.config_source.load(&overrides)?;
            Ok((overrides, config))
        });
        match changed {
            Ok((overrides, config)) => {
                self.overrides = overrides;
                self.apply_config(config);
            }
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                "the setting was not changed"
            ),
        }
    }

    /// Makes `config` the configuration in effect, changing what differs.
    fn apply_config(&mut self, config: Config) {
        // Menus are made from the configuration being replaced.
        self.menus.close();
        self.menus.set_search(config.settings.menus.search);
        self.window_manager.apply_settings(&config.settings);
        self.taskbars.apply(&config.settings.taskbar);
        if config.theme != self.config.theme {
            self.window_manager.restyle(config.theme.metrics());
            for taskbar in self.taskbars.iter_mut() {
                taskbar.invalidate();
            }
        }
        let output_changes = (config.outputs != self.config.outputs)
            .then(|| self.outputs.configure(&config.outputs, &mut self.backend));
        self.backdrop.apply(&config.settings.wallpaper);
        if let Backend::Drm(backend) = &mut self.backend {
            backend.set_input_settings(&config.settings.input);
        }
        // A reload is when icons installed or changed since show.
        self.icons.forget();
        for taskbar in self.taskbars.iter_mut() {
            taskbar.forget_icons();
        }
        self.input.bindings_changed(&config.bindings);
        self.config = config;
        self.start_taskbar_services();
        if let Some(changes) = output_changes {
            self.follow_output_changes(&changes);
        }
    }

    /// Follows udev's news of a GPU on the DRM backend: added, changed (a
    /// monitor plugged or unplugged) or removed.
    pub(crate) fn drm_device_event(&mut self, event: smithay::backend::udev::UdevEvent) {
        let events = match &mut self.backend {
            Backend::Drm(backend) => backend.device_event(event),
            Backend::Winit(_) | Backend::Headless(_) => Vec::new(),
        };
        self.handle_output_events(events);
    }

    /// Follows the seat taking the devices away (`is_active` false: another
    /// terminal is shown) or giving them back.
    pub(crate) fn drm_session(&mut self, is_active: bool) {
        let events = match &mut self.backend {
            Backend::Drm(backend) if is_active => backend.resume(),
            Backend::Drm(backend) => {
                backend.pause();
                Vec::new()
            }
            Backend::Winit(_) | Backend::Headless(_) => Vec::new(),
        };
        self.handle_output_events(events);
    }

    /// Follows a monitor showing its frame on GPU `node`'s CRTC `crtc`, as
    /// its page flip's `metadata` says.
    pub(crate) fn drm_frame_shown(
        &mut self,
        node: smithay::backend::drm::DrmNode,
        crtc: smithay::reexports::drm::control::crtc::Handle,
        metadata: Option<&smithay::backend::drm::DrmEventMetadata>,
    ) {
        if let Backend::Drm(backend) = &mut self.backend {
            backend.frame_shown(node, crtc, metadata);
            if let Some(output) = backend.output_on(node, crtc) {
                self.output_refreshed(&output);
            }
        }
    }

    /// Follows a monitor's wait after an unchanged frame ending, on GPU
    /// `node`'s CRTC `crtc`.
    pub(crate) fn drm_wait_over(
        &mut self,
        node: smithay::backend::drm::DrmNode,
        crtc: smithay::reexports::drm::control::crtc::Handle,
    ) {
        if let Backend::Drm(backend) = &mut self.backend {
            backend.wait_over(node, crtc);
            if let Some(output) = backend.output_on(node, crtc) {
                self.output_refreshed(&output);
            }
        }
    }

    /// Follows `output` refreshing: the frames of clients using fifo that
    /// waited for it go on.
    fn output_refreshed(&mut self, output: &Output) {
        let roots = self.client_roots();
        let released = FramePacing::output_refreshed(&roots, output);
        self.let_frames_go_on(released);
    }

    /// Follows commits that may have asked for frames at a moment: wakes
    /// for the earliest.
    pub(crate) fn follow_timed_frames(&mut self) {
        let deadline = FramePacing::next_deadline(&self.client_roots());
        let interval = crate::presentation::frame_interval(self.outputs.primary());
        self.frame_pacing
            .wake_for(deadline, interval, (&self.loop_handle, &self.ledger));
    }

    /// Lets the frames `clients` held back go on: their commits waiting on
    /// a barrier lifted are applied.
    fn let_frames_go_on(&mut self, clients: Vec<smithay::reexports::wayland_server::Client>) {
        let display = self.display_handle.clone();
        for client in clients {
            smithay::wayland::compositor::CompositorHandler::client_compositor_state(self, &client)
                .blocker_cleared(self, &display);
        }
    }

    /// Returns the root surface of everything clients may show, shown or
    /// hidden, each with its subsurfaces beneath it: windows, X programs'
    /// menus, layer surfaces, lock surfaces, and their popups.
    fn client_roots(&self) -> Vec<WlSurface> {
        let mut roots: Vec<WlSurface> = self.toplevels.surfaces().collect();
        if let Some(xwayland) = &self.xwayland {
            roots.extend(xwayland.unmanaged().filter_map(X11Surface::wl_surface));
        }
        roots.extend(
            self.layers
                .iter()
                .map(|(layer, _)| layer.wl_surface().clone()),
        );
        roots.extend(
            self.session_lock
                .surfaces()
                .map(|(_, surface)| surface.clone()),
        );
        let popups: Vec<WlSurface> = roots
            .iter()
            .flat_map(PopupManager::popups_for_surface)
            .map(|(popup, _)| popup.wl_surface().clone())
            .collect();
        roots.extend(popups);
        roots
    }

    /// Follows the headless backend's outputs file (at `path`) changing:
    /// simulated monitors plugged in, unplugged or changed.
    pub(crate) fn reread_headless_outputs(&mut self, path: &Path) {
        let events = match &mut self.backend {
            Backend::Headless(backend) => backend.reread_output_file(path),
            Backend::Winit(_) | Backend::Drm(_) => Vec::new(),
        };
        self.handle_output_events(events);
    }

    /// Tells the surface under the pointer how the mouse moved, before any
    /// constraint or edge holds the pointer: what a game aiming reads.
    fn send_relative_motion(&mut self, event: &RelativeMotionEvent) {
        let from = self.pointer_location();
        let under = self
            .scene()
            .target_at(from)
            .and_then(|target| target.client_surface());
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        pointer.relative_motion(self, under, event);
    }

    /// Returns where the pointer at `from` goes when the mouse moves it by
    /// `delta`: there, unless a constraint holds it (`pointer_lock`).
    fn constrained_motion(
        &self,
        from: Point<f64, Logical>,
        delta: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        let Some((surface, origin)) = self
            .scene()
            .target_at(from)
            .and_then(|target| target.client_surface())
        else {
            return from + delta;
        };
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let active = with_pointer_constraint(&surface, &pointer, |constraint| {
            let constraint = constraint.filter(|constraint| constraint.is_active())?;
            let hold = match &*constraint {
                PointerConstraint::Locked(_) => PointerHold::Locked,
                PointerConstraint::Confined(_) => PointerHold::Confined,
            };
            Some((hold, constraint.region().cloned()))
        });
        let Some((hold, region)) = active else {
            return from + delta;
        };
        let scene = self.scene();
        pointer_lock::constrain(hold, from, delta, |point| {
            let on_surface = (point - origin).to_i32_round();
            region
                .as_ref()
                .is_none_or(|region| region.contains(on_surface))
                && scene
                    .target_at(point)
                    .and_then(|target| target.client_surface())
                    .is_some_and(|(under, _)| under == surface)
        })
    }

    /// Lets the constraint of the surface under the pointer at `location`
    /// hold the pointer, if its surface has the keyboard.
    fn activate_pointer_constraint_under(&mut self, location: Point<f64, Logical>) {
        if let Some((surface, _)) = self
            .scene()
            .target_at(location)
            .and_then(|target| target.client_surface())
        {
            self.activate_pointer_constraint(&surface);
        }
    }

    /// Lets `surface`'s pointer constraint hold the pointer, if `surface`
    /// is under the pointer and has the keyboard: a game in the
    /// background never takes the pointer.
    pub(crate) fn activate_pointer_constraint(&mut self, surface: &WlSurface) {
        let has_keyboard = self
            .keyboard_focus
            .as_ref()
            .and_then(FocusTarget::surface)
            .is_some_and(|focused| focused == *surface);
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let is_under = pointer
            .current_focus()
            .is_some_and(|under| under == *surface);
        if has_keyboard && is_under {
            with_pointer_constraint(surface, &pointer, |constraint| {
                if let Some(constraint) = constraint.filter(|constraint| !constraint.is_active()) {
                    constraint.activate();
                    tracing::debug!("pointer constraint active");
                }
            });
        }
    }

    /// Lets go of `surface`'s pointer constraint: it lost the keyboard.
    fn deactivate_pointer_constraint(&mut self, surface: &WlSurface) {
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        with_pointer_constraint(surface, &pointer, |constraint| {
            if let Some(constraint) = constraint.filter(|constraint| constraint.is_active()) {
                constraint.deactivate();
                tracing::debug!("pointer constraint released");
            }
        });
        self.end_pointer_lock(surface);
    }

    /// Puts the pointer where the game whose lock on `surface` ended last
    /// said it is, if it said.
    pub(crate) fn end_pointer_lock(&mut self, surface: &WlSurface) {
        let Some((hinted, location)) = self.pointer_lock_hint.take() else {
            return;
        };
        if hinted != *surface {
            self.pointer_lock_hint = Some((hinted, location));
            return;
        }
        let origin = self
            .scene()
            .target_at(self.pointer_location())
            .and_then(|target| target.client_surface())
            .filter(|(under, _)| under == surface)
            .map(|(_, origin)| origin);
        if let Some(origin) = origin {
            self.move_pointer_to(origin + location, InputTime::now());
        }
    }

    /// Returns whether the surface with the keyboard takes every key
    /// (`keyboard-shortcuts-inhibit`), so vaht's bindings rest.
    fn are_shortcuts_inhibited(&self) -> bool {
        self.keyboard_focus
            .as_ref()
            .and_then(FocusTarget::surface)
            .and_then(|surface| self.seat.keyboard_shortcuts_inhibitor_for_surface(&surface))
            .is_some_and(|inhibitor| inhibitor.is_active())
    }

    /// Tells idle whether a visible surface asks that the session not go
    /// idle: a shown window's, or a mapped layer surface's.
    pub(crate) fn refresh_idle_inhibition(&mut self) {
        let shown: Vec<WindowId> = self
            .window_manager
            .shown_top_down()
            .map(Window::id)
            .collect();
        let toplevels = &self.toplevels;
        let layers = &self.layers;
        let is_visible = |surface: &WlSurface| {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            toplevels
                .window_of(&root)
                .is_some_and(|window| shown.contains(&window))
                || layers.find(&root).is_some()
        };
        let is_inhibited = self
            .idle_inhibitors
            .is_inhibiting(WlSurface::is_alive, is_visible);
        self.protocols.idle_notifier.set_is_inhibited(is_inhibited);
    }

    /// Starts Xwayland, unless the settings, or `allows`, turn it off.
    fn start_xwayland(&mut self, allows: bool) {
        if !allows {
            tracing::info!("Xwayland is off (--no-xwayland)");
            return;
        }
        if !self.config.settings.xwayland.enabled {
            tracing::info!("Xwayland is off ([xwayland] enabled)");
            return;
        }
        match InputEmulation::start(&self.loop_handle, &self.ledger) {
            Ok(emulation) => self.input_emulation = Some(emulation),
            Err(error) => tracing::warn!(error, "no input emulation for X programs"),
        }
        let socket = self.input_emulation.as_ref().map(InputEmulation::socket);
        match XwaylandServer::start(
            &self.loop_handle,
            &self.display_handle,
            &self.ledger,
            socket,
        ) {
            Ok(server) => self.xwayland = Some(server),
            Err(error) => tracing::warn!(error, "no X11 programs"),
        }
    }

    /// Becomes the window manager of Xwayland, ready on `socket` for
    /// `display_number`, whose Wayland client is `client`; programs started
    /// from now on, the autostart programs first, find it in `DISPLAY`.
    pub(crate) fn xwayland_ready(
        &mut self,
        socket: std::os::unix::net::UnixStream,
        display_number: u32,
        client: smithay::reexports::wayland_server::Client,
    ) {
        let mut wm = match X11Wm::start_wm(
            self.loop_handle.clone(),
            &self.display_handle,
            socket,
            client,
        ) {
            Ok(wm) => wm,
            Err(error) => {
                tracing::warn!(%error, "could not become Xwayland's window manager");
                self.finish_xwayland_start();
                return;
            }
        };
        let frame = self.cursor.default_frame();
        let narrow = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
        if let Err(error) = wm.set_cursor(
            &frame.pixels,
            (narrow(frame.size.0), narrow(frame.size.1)).into(),
            (narrow(frame.hotspot.0), narrow(frame.hotspot.1)).into(),
        ) {
            tracing::debug!(%error, "X programs keep Xwayland's own pointer");
        }
        self.load_x_resources(display_number);
        if let Some(server) = &mut self.xwayland {
            server.manage(wm, display_number, (&self.loop_handle, &self.ledger));
            if let Some(name) = server.display_name() {
                self.launcher.set_environment("DISPLAY".to_owned(), name);
            }
        }
        // Services and programs started from now on find X too.
        self.finish_xwayland_start();
    }

    /// Tells the D-Bus and systemd activation environment of the session,
    /// or empties it there (`is_clearing`), if vaht exports it.
    fn update_activation_environment(&mut self, is_clearing: bool) {
        if let Some(command) = self.activation_update(is_clearing) {
            self.launcher.run(&command);
        }
    }

    /// Returns the command that tells the activation environment of the
    /// session, or empties it there (`is_clearing`), if vaht exports it.
    fn activation_update(&self, is_clearing: bool) -> Option<String> {
        if !self.exports_activation_environment {
            return None;
        }
        let launcher = &self.launcher;
        let command =
            session_environment::update_command(is_clearing, |name| launcher.is_set(name));
        tracing::info!(command, "activation environment updated");
        Some(command)
    }

    /// Loads the X resources file the settings name into Xwayland on
    /// `display_number`, before programs started from now on read it.
    fn load_x_resources(&self, display_number: u32) {
        /// The largest resources file loaded, as fluxland had it.
        const MAX_BYTES: u64 = 1024 * 1024;
        let Some(path) = self
            .config
            .settings
            .xwayland
            .resources_path(self.config_source.directory())
        else {
            return;
        };
        let resources = std::fs::File::open(&path).and_then(|file| {
            let mut text = Vec::new();
            file.take(MAX_BYTES).read_to_end(&mut text).map(|_| text)
        });
        let loaded = resources
            .map_err(|error| error.to_string())
            .and_then(|text| XwaylandServer::load_resources(&format!(":{display_number}"), &text));
        match loaded {
            Ok(()) => tracing::info!(path = %path.display(), "X resources loaded"),
            Err(error) => tracing::warn!(error, path = %path.display(), "X resources not loaded"),
        }
    }

    /// Starts managing the X11 window `surface`, which asked to be mapped.
    pub(crate) fn add_x11_window(&mut self, surface: X11Surface) {
        let window = self
            .window_manager
            .add_window(&self.config.settings, &self.ledger);
        self.toplevels
            .insert(window, Toplevel::from_x11(surface), &self.ledger);
        // An X11 window's size and class are known already: it is placed
        // now, as an xdg-shell window is at its first commit.
        self.commit_toplevel(window);
        self.schedule_render();
    }

    /// Stops showing the X11 window `surface`, unmapped or gone.
    pub(crate) fn forget_x11_window(&mut self, surface: &X11Surface) {
        if self
            .xwayland
            .as_mut()
            .is_some_and(|server| server.forget_unmanaged(surface))
        {
            self.schedule_render();
            return;
        }
        if let Some(server) = &mut self.xwayland {
            server.untrack_shape(surface);
        }
        if let Some(window) = self.toplevels.remove_x11(surface) {
            self.window_manager.remove_window(window);
            self.settle();
            self.schedule_render();
        }
    }

    /// Reads the X11 windows' shapes that changed, and draws again if any
    /// did.
    pub(crate) fn xwayland_shapes_changed(&mut self) {
        let toplevels = &self.toplevels;
        let windows: Vec<X11Surface> = self
            .window_manager
            .windows_top_down()
            .filter_map(|window| toplevels.get(window.id())?.x11().cloned())
            .collect();
        let Some(server) = &mut self.xwayland else {
            return;
        };
        let unmanaged: Vec<X11Surface> = server.unmanaged().cloned().collect();
        let size_of = |id: u32| {
            windows
                .iter()
                .chain(&unmanaged)
                .find(|window| window.window_id() == id)
                .map(|window| window.last_configure().size)
        };
        if server.follow_shapes(size_of) {
            self.schedule_render();
        }
    }

    /// Follows the X11 window `surface`'s title, class or decorations hint
    /// changing.
    pub(crate) fn x11_properties_changed(&mut self, surface: &X11Surface) {
        if let Some(window) = self.toplevels.window_of_x11(surface) {
            self.commit_toplevel(window);
        }
    }

    /// Answers the X11 window `surface` asking to cover `asked`: a window
    /// vaht does not manage yet goes where it asks; a managed floating one
    /// takes the size it asks where it is; any other keeps its place.
    pub(crate) fn answer_x11_configure(
        &mut self,
        surface: &X11Surface,
        asked: Rectangle<i32, Logical>,
    ) {
        let Some(window) = self.toplevels.window_of_x11(surface) else {
            if let Err(error) = surface.configure(asked) {
                tracing::debug!(%error, "an X11 window could not be configured");
            }
            return;
        };
        let Some(managed) = self.window_manager.window(window) else {
            return;
        };
        let area = if managed.placement().kind() == PlacementKind::Floating {
            Rectangle::new(managed.content_area().loc, asked.size)
        } else {
            managed.content_area()
        };
        // X expects an answer, even one that changes nothing.
        if let Err(error) = surface.configure(area) {
            tracing::debug!(%error, "an X11 window could not be configured");
        }
    }

    /// Notes that the X server has the X11 window `surface`'s content at
    /// `origin`; if the pointer is on it, tells the window where the pointer
    /// is again, now that the X server agrees where the window is.
    pub(crate) fn x11_position_confirmed(
        &mut self,
        surface: &X11Surface,
        origin: Point<i32, Logical>,
    ) {
        let Some(window) = self.toplevels.window_of_x11(surface) else {
            return;
        };
        let Some(toplevel) = self.toplevels.get_mut(window) else {
            return;
        };
        toplevel.confirm_position(origin);
        let own = toplevel.wl_surface();
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let is_pointed_at = own.is_some_and(|own| {
            pointer.current_focus().as_ref() == Some(&own)
                || pointer
                    .grab_start_data()
                    .and_then(|start| start.focus)
                    .is_some_and(|(focus, _)| focus == own)
        });
        if is_pointed_at {
            self.move_pointer_to(self.pointer_location(), InputTime::now());
        }
    }

    /// Returns where the pointer goes during a drag of `window` that began
    /// at `start`: for an X11 window dragged from its own surface (it asked
    /// to move), its surface, relative to where the X server has it; none
    /// for any other drag.
    pub(crate) fn x11_drag_focus(
        &self,
        window: WindowId,
        start: &GrabStartData<Self>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let toplevel = self.toplevels.get(window)?;
        toplevel.x11()?;
        let surface = toplevel.wl_surface()?;
        let (pressed, _) = start.focus.as_ref()?;
        if *pressed != surface {
            return None;
        }
        let placed = self.window_manager.window(window)?.surface_origin();
        Some((surface, toplevel.pointer_origin(placed).to_f64()))
    }

    /// Passes the X11 window `surface`'s `request` on to the window manager.
    pub(crate) fn honor_x11_request(&mut self, surface: &X11Surface, request: ClientRequest) {
        if let Some(window) = self.toplevels.window_of_x11(surface) {
            self.window_manager.honor(window, request);
            self.settle();
        }
    }

    /// Starts `drag` on the X11 window `surface`, as its program asks (its
    /// own title bar dragged), while a button pressed on it is held.
    pub(crate) fn start_x11_drag(&mut self, surface: &X11Surface, drag: WindowDrag) {
        let Some(window) = self.toplevels.window_of_x11(surface) else {
            return;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some(start) = pointer.grab_start_data() else {
            return;
        };
        let is_on_window = start.focus.as_ref().is_some_and(|(focus, _)| {
            self.toplevels
                .get(window)
                .is_some_and(|toplevel| toplevel.has_surface(focus))
        });
        if is_on_window {
            self.start_window_grab(window, drag, start, SERIAL_COUNTER.next_serial());
        }
    }

    /// Brings the X11 window `surface` forward, as its program asks, if
    /// its focus-stealing rule allows: an X program moving between its own
    /// windows (the one active is an X11 window) is asked for.
    pub(crate) fn activate_x11_window(
        &mut self,
        surface: &X11Surface,
        currently_active: Option<&X11Surface>,
    ) {
        let Some(window) = self.toplevels.window_of_x11(surface) else {
            return;
        };
        let is_asked_for =
            currently_active.is_some() && matches!(self.keyboard_focus, Some(FocusTarget::X11(_)));
        let policy = self
            .window_manager
            .window(window)
            .map_or(FocusStealing::Default, Window::focus_stealing);
        if policy.honors(is_asked_for) {
            self.window_manager.activate(window);
            self.settle();
        }
    }

    /// Follows the backend's outputs changing, as `events` say.
    pub(crate) fn handle_output_events(&mut self, events: Vec<OutputEvent>) {
        for event in events {
            let settings = &self.config.outputs;
            let changes = match event {
                OutputEvent::Connected(output) => {
                    let changes = self
                        .outputs
                        .connect(output.clone(), settings, &mut self.backend);
                    self.follow_output_changes(&changes);
                    self.bring_windows_back_to(&output);
                    continue;
                }
                OutputEvent::Disconnected(output) => {
                    self.output_controls.power.output_gone(&output);
                    self.output_controls.gamma.output_gone(&output);
                    self.remember_windows_on(&output);
                    let changes =
                        self.outputs
                            .disconnect(&output, &self.config.outputs, &mut self.backend);
                    self.follow_output_changes(&changes);
                    let metrics = self.window_manager.metrics();
                    let window_manager = &self.window_manager;
                    self.output_memory.note_brought(|window| {
                        window_manager
                            .window(window)
                            .map(|window| window.outer_area(metrics))
                    });
                    continue;
                }
                OutputEvent::Changed(output) => {
                    tracing::info!(output = output.name(), "output changed");
                    self.outputs.configure(settings, &mut self.backend)
                }
            };
            self.follow_output_changes(&changes);
        }
    }

    /// Remembers where the windows on `output`, which is leaving, are.
    fn remember_windows_on(&mut self, output: &Output) {
        let metrics = self.window_manager.metrics();
        let windows: Vec<_> = self
            .window_manager
            .windows_top_down()
            .filter(|window| window.is_placed())
            .map(|window| (window.id(), window.outer_area(metrics)))
            .filter(|(_, area)| self.outputs.most_overlapping(*area) == Some(output))
            .collect();
        self.output_memory
            .output_leaving(&output.name(), Outputs::geometry(output).loc, windows);
    }

    /// Puts back on `output`, plugged in again, the windows that were on it
    /// and have not been moved since.
    fn bring_windows_back_to(&mut self, output: &Output) {
        let geometry = Outputs::geometry(output);
        let metrics = self.window_manager.metrics();
        let window_manager = &self.window_manager;
        let back = self
            .output_memory
            .output_back(&output.name(), geometry.loc, |window| {
                window_manager
                    .window(window)
                    .map(|window| window.outer_area(metrics))
            });
        if back.is_empty() {
            return;
        }
        let Some(area) = self
            .output_areas()
            .iter()
            .copied()
            .find(|area| area.whole.loc == geometry.loc)
        else {
            return;
        };
        for (window, corner) in back {
            tracing::debug!(%window, output = output.name(), "window back on its output");
            self.window_manager.return_to(window, area, corner);
        }
        self.settle();
        self.schedule_render();
    }

    /// Follows the outputs being set up again: layer surfaces on outputs
    /// gone are closed, windows are placed again, the taskbar is laid out
    /// anew, and the hooks are told of `changes`.
    fn follow_output_changes(&mut self, changes: &OutputChanges) {
        self.layers.close_off(&self.outputs);
        for taskbar in self.taskbars.iter_mut() {
            taskbar.invalidate();
        }
        let hooks = &self.config.outputs.hooks;
        if let Some(profile) = &changes.applied_profile {
            tracing::info!(profile, "output profile applied");
            if let Some(hook) = &hooks.profile_applied {
                self.launcher.run_hook(hook, profile);
            }
        }
        if let Some(output) = &changes.primary
            && let Some(hook) = &hooks.primary_output_changed
        {
            self.launcher.run_hook(hook, output);
        }
        self.refresh_layer_keyboard();
        self.settle();
        self.schedule_render();
    }

    /// Asks for a frame to be drawn soon.
    pub(crate) fn schedule_render(&mut self) {
        self.backend.schedule_render();
    }

    /// Draws a frame, then tells the windows and layer surfaces in it that
    /// they were shown; hidden windows get no frame callbacks, so they stop
    /// drawing.
    pub(crate) fn render(&mut self) {
        // Timed frames due by the time this frame shows are let in first.
        let interval = crate::presentation::frame_interval(self.outputs.primary());
        let released =
            FramePacing::release_due(&self.client_roots(), crate::presentation::now() + interval);
        self.let_frames_go_on(released);
        self.prepare_frames();
        self.update_vrr();
        let scene = Scene::new(
            &self.window_manager,
            &self.toplevels,
            Drawn {
                frames: &self.frame_textures,
                taskbars: &self.taskbars,
                menus: &self.menus,
                backdrop: &self.backdrop,
                outline: &self.outline,
                tooltip: &self.tooltip,
                snap: &self.snap_overlay,
                xwayland: self.xwayland.as_ref(),
                lock: &self.session_lock,
            },
            LayerMaps::new(&self.outputs),
        );
        let draws_pointer = self.backend.draws_pointer();
        let scene = if draws_pointer {
            scene.with_pointer(&self.cursor, self.pointer_location())
        } else {
            scene
        };
        if let Err(error) = self.backend.render(&scene) {
            tracing::warn!(error = &error as &dyn std::error::Error, "frame skipped");
        }
        let time = self.start_time.elapsed();
        for (window, toplevel) in scene.windows() {
            let area = window.content_area();
            toplevel.enter_outputs(&self.outputs, area, window.surface_origin());
            let shown_on = self
                .outputs
                .most_overlapping(area)
                .unwrap_or_else(|| self.outputs.primary());
            toplevel.send_frame(shown_on, time);
        }
        for (layer, output) in self.layers.iter() {
            layer.send_frame(output, time, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
        // What shows changed, and with it whether idle is inhibited.
        self.refresh_idle_inhibition();
        // A frame showing nothing of the session went out: the locker
        // hears the session is locked.
        if self.session_lock.is_locked() {
            self.session_lock.confirm();
            for (output, surface) in self.session_lock.surfaces() {
                send_frames_surface_tree(surface, output, time, Some(Duration::ZERO), |_, _| {
                    Some(output.clone())
                });
            }
        }
        // An animated pointer moves on by itself.
        if draws_pointer && self.cursor.is_animated() {
            self.schedule_render();
        }
        // A monitor refreshes when its page flips; other outputs refresh as
        // soon as they are drawn.
        if !matches!(self.backend, Backend::Drm(_)) {
            for output in self.outputs.iter().cloned().collect::<Vec<_>>() {
                self.output_refreshed(&output);
            }
        }
        self.follow_timed_frames();
        // Windows opened, renamed or closed are announced as they show.
        let windows = self
            .window_manager
            .windows_top_down()
            .map(|window| (window.id(), window.title(), window.app_id()));
        self.toplevel_list.follow(
            windows,
            &mut ForeignToplevels {
                state: &mut self.protocols.foreign_toplevel_list,
            },
        );
        let facts = self.window_facts();
        self.toplevel_management.follow(facts, &self.display_handle);
        tracing::trace!("frame drawn");
    }

    /// Returns what programs managing windows are told of each placed
    /// window, in the order they opened.
    fn window_facts(&self) -> Vec<(WindowId, WindowFacts<Output>)> {
        let focused = self.window_manager.focused();
        let mut facts: Vec<_> = self
            .window_manager
            .windows_top_down()
            .filter(|window| window.is_placed())
            .map(|window| {
                let area = window.content_area();
                let states = ToplevelStates {
                    covering: match window.placement() {
                        Placement::Maximized { .. } => Covering::Maximized,
                        Placement::Fullscreen { .. } => Covering::Fullscreen,
                        Placement::Floating | Placement::Tiled { .. } => Covering::Placed,
                    },
                    is_minimized: window.state().is_minimized,
                    is_activated: focused == Some(window.id()),
                };
                let outputs = self
                    .outputs
                    .iter()
                    .filter(|output| Outputs::geometry(output).overlaps(area))
                    .cloned()
                    .collect();
                let facts = WindowFacts {
                    title: window.title().to_owned(),
                    app_id: window.app_id().to_owned(),
                    states,
                    outputs,
                };
                (window.id(), facts)
            })
            .collect();
        facts.sort_by_key(|(window, _)| *window);
        facts
    }

    /// Carries out what a program managing windows asked of `window`.
    pub(crate) fn carry_out_toplevel_request(&mut self, window: WindowId, asked: &HandleRequest) {
        let Some(target) = self.window_manager.window(window) else {
            return;
        };
        let placement = target.placement();
        let is_maximized = matches!(placement, Placement::Maximized { .. });
        let is_fullscreen = matches!(placement, Placement::Fullscreen { .. });
        let is_minimized = target.state().is_minimized;
        let action = match asked {
            HandleRequest::Activate | HandleRequest::Minimize(false) => {
                self.window_manager.activate(window);
                None
            }
            HandleRequest::Minimize(true) => {
                (!is_minimized).then_some(WindowManagerAction::Minimize)
            }
            HandleRequest::Maximize(wants) => {
                (*wants != is_maximized).then_some(WindowManagerAction::Maximize(Axes::Both))
            }
            HandleRequest::Fullscreen(wants) => {
                (*wants != is_fullscreen).then_some(WindowManagerAction::Fullscreen)
            }
            HandleRequest::Close => Some(WindowManagerAction::Close),
        };
        if let Some(action) = action {
            self.window_manager.run(&action, Some(window));
        }
        self.schedule_render();
    }

    /// Turns each monitor's variable refresh on or off as its `vrr` setting
    /// says, with a fullscreen window on top there or not.
    fn update_vrr(&mut self) {
        let Backend::Drm(backend) = &mut self.backend else {
            return;
        };
        for output in self.outputs.iter() {
            let area = Outputs::geometry(output);
            let is_fullscreen = self
                .window_manager
                .shown_top_down()
                .find(|window| {
                    self.outputs
                        .most_overlapping(window.content_area())
                        .is_some_and(|on| on == output)
                        && area.intersection(window.content_area()).is_some()
                })
                .is_some_and(|window| window.placement().is_fullscreen());
            let top = if is_fullscreen {
                TopWindow::Fullscreen
            } else {
                TopWindow::Other
            };
            let vrr = self.outputs.vrr_of(output, &self.config.outputs);
            backend.set_vrr(output, vrr, top);
        }
    }

    /// Brings every frame's pixels up to date with what it shows; drawing a
    /// frame and taking a screenshot both start here, so a screenshot shows
    /// the same as the screen would.
    pub(crate) fn prepare_frames(&mut self) {
        self.window_manager
            .paint_frames(&self.config.theme, (&mut self.typesetter, &mut self.icons));
        self.frame_textures
            .update(&self.window_manager, &self.config.theme, &self.ledger);
        self.sync_taskbars();
        let outputs = &self.outputs;
        self.taskbars.prepare(
            |name| {
                outputs
                    .named(name)
                    .map_or_else(|| outputs.primary_geometry(), Outputs::geometry)
            },
            (&self.window_manager, &self.config.settings.taskbar),
            &self.config.theme,
            (&mut self.typesetter, &mut self.icons),
        );
        self.menus.prepare(&self.config.theme, &mut self.typesetter);
        let output_areas: Vec<_> = self.outputs.iter().map(Outputs::geometry).collect();
        self.backdrop.prepare(&output_areas, &self.window_manager);
        self.outline
            .update(self.window_manager.outline(), &self.config.theme);
        self.snap_overlay.update(&self.window_manager);
        self.cursor.prepare(self.start_time.elapsed());
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
        if self.layer_maps().commit(surface) {
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
        self.layers.unmap(surface);
        self.refresh_layer_keyboard();
        self.refresh_output_area();
        self.schedule_render();
    }

    /// Delivers one input event; absolute motion spans every output.
    pub(crate) fn handle_input<B: InputBackend>(&mut self, event: InputEvent<B>) {
        self.handle_input_within(event, None);
    }

    /// Tells pagers of the workspaces, if anything about them changed.
    fn announce_workspaces(&mut self) {
        let current = self.window_manager.current_workspace();
        let last = self.window_manager.workspaces().count();
        let workspaces = self
            .window_manager
            .workspaces()
            .enumerate()
            .map(|(index, (workspace, name))| WorkspaceFacts {
                name: name.to_owned(),
                is_active: workspace == current,
                is_last: index + 1 == last,
            })
            .collect();
        let outputs = self.outputs.iter().cloned().collect();
        self.protocols
            .workspace_pager
            .follow(workspaces, outputs, &self.display_handle);
    }

    /// Carries out what a pager asked at its commit, in order.
    pub(crate) fn carry_out_workspace_requests(&mut self, asked: Vec<WorkspaceRequest>) {
        for request in asked {
            match request {
                WorkspaceRequest::Activate(index) => {
                    if let Some(number) = std::num::NonZeroUsize::new(index + 1) {
                        self.window_manager.run(
                            &WindowManagerAction::Workspace(WorkspaceTarget::Numbered(number)),
                            None,
                        );
                    }
                }
                WorkspaceRequest::Remove(index) => {
                    if index + 1 == self.window_manager.workspaces().count() {
                        self.window_manager
                            .run(&WindowManagerAction::RemoveLastWorkspace, None);
                    }
                }
                WorkspaceRequest::Create(name) => self.window_manager.add_workspace_named(&name),
            }
        }
        self.schedule_render();
    }

    /// Tells programs arranging outputs of the outputs, if anything about
    /// them changed.
    fn announce_outputs(&mut self) {
        let connected = self
            .outputs
            .connected()
            .map(|output| {
                let is_enabled = self.outputs.iter().any(|on| on == output);
                (output.clone(), HeadState::of(output, is_enabled))
            })
            .collect();
        self.output_controls
            .management
            .follow(connected, &self.display_handle);
    }

    /// Arranges the outputs as a program asked: each of `wanted` on, as
    /// asked, or off; or only checks that it could (`is_test`). Returns
    /// whether it could: at least one output stays on, each mode asked
    /// for is one the output has, and each scale is from 0.25 to 8.
    pub(crate) fn apply_output_configuration(
        &mut self,
        wanted: &[(Output, Option<HeadRequest>)],
        is_test: bool,
    ) -> bool {
        if wanted.iter().all(|(_, asked)| asked.is_none()) {
            return false;
        }
        let mut entries = Vec::new();
        for (output, asked) in wanted {
            let mut entry = OutputEntry::for_connector(&output.name());
            entry.primary = output == self.outputs.primary();
            let identity = Outputs::identity(output);
            if let Some(written) = self.config.outputs.entry_for(identity.as_identity()) {
                entry.vrr = written.vrr;
            }
            let Some(asked) = asked else {
                entry.enabled = false;
                entries.push(entry);
                continue;
            };
            let scale = asked
                .scale
                .unwrap_or_else(|| output.current_scale().fractional_scale());
            let Some(scale) = OutputScale::new(scale) else {
                return false;
            };
            entry.scale = Some(scale);
            let mode = asked.mode.or_else(|| {
                output
                    .current_mode()
                    .map(|mode| (mode.size.w, mode.size.h, Some(mode.refresh)))
            });
            if let Some((width, height, refresh)) = mode {
                let is_offered = output.modes().iter().any(|offered| {
                    offered.size.w == width
                        && offered.size.h == height
                        && refresh.is_none_or(|refresh| offered.refresh == refresh)
                });
                if !is_offered {
                    return false;
                }
                entry.mode = Some(OutputMode {
                    width,
                    height,
                    refresh_millihertz: refresh,
                });
            }
            let position = asked.position.unwrap_or_else(|| output.current_location());
            entry.position = Some([position.x, position.y]);
            entry.transform = OutputTransform::from_smithay(
                asked
                    .transform
                    .unwrap_or_else(|| output.current_transform()),
            );
            entries.push(entry);
        }
        if is_test {
            return true;
        }
        let settings = self.config.outputs.exactly(entries);
        let changes = self.outputs.arrange_exactly(&settings, &mut self.backend);
        self.follow_output_changes(&changes);
        tracing::info!("outputs arranged as a program asked");
        true
    }

    /// Notes that the user did something: idle timers start over, and
    /// outputs turned off come on again.
    fn note_activity(&mut self) {
        self.protocols.idle_notifier.notify_activity(&self.seat);
        for output in self.output_controls.power.wake_all() {
            tracing::info!(output = output.name(), "output woken by input");
            self.power_backend(&output, true);
        }
    }

    /// Turns `output` on or off (`is_on`), as a program asked.
    pub(crate) fn set_output_power(&mut self, output: &Output, is_on: bool) {
        if self.output_controls.power.set(output, is_on) {
            tracing::info!(output = output.name(), is_on, "output power changed");
            self.power_backend(output, is_on);
        }
    }

    /// Wakes or sleeps `output`'s monitor (`is_on`), where there is one.
    fn power_backend(&mut self, output: &Output, is_on: bool) {
        if let Backend::Drm(backend) = &mut self.backend {
            backend.set_power(output, is_on);
        }
        self.schedule_render();
    }

    /// Delivers one input event; absolute motion spans `absolute_area`
    /// (global coordinates), or every output without it.
    pub(crate) fn handle_input_within<B: InputBackend>(
        &mut self,
        event: InputEvent<B>,
        absolute_area: Option<Rectangle<i32, Logical>>,
    ) {
        // Any input is the user being there.
        self.note_activity();
        // A key typed right after a binding moved focus goes where it went.
        self.settle();
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "the rest go to touch::handle, tablet::handle and gestures::forward, \
                      which take touches, tablets and gestures"
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
                let delta = event.delta();
                self.send_relative_motion(&RelativeMotionEvent {
                    delta,
                    delta_unaccel: event.delta_unaccel(),
                    time: event.time(),
                });
                let from = self.pointer_location();
                let to = self.constrained_motion(from, delta);
                let location = self.outputs.clamp(from, to);
                self.move_pointer_to(location, event.time());
                self.activate_pointer_constraint_under(location);
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let area = absolute_area.unwrap_or_else(|| self.outputs.bounds());
                let location = event.position_transformed(area.size) + area.loc.to_f64();
                self.move_pointer_to(location, event.time());
            }
            InputEvent::PointerButton { event } => self.handle_pointer_button::<B>(&event),
            InputEvent::PointerAxis { event } => self.handle_pointer_axis::<B>(&event),
            InputEvent::SwitchToggle { event } => {
                let switch = match event.switch() {
                    Some(Switch::Lid) => Some(SwitchKind::Lid),
                    Some(Switch::TabletMode) => Some(SwitchKind::TabletMode),
                    // A switch libinput knows and vaht does not.
                    _ => None,
                };
                if let Some(switch) = switch {
                    let flip = SwitchFlip::new(switch, event.state() == SwitchState::On);
                    tracing::info!(?flip, "switch flipped");
                    if let Some(command) = self.config.bindings.switch(flip).cloned() {
                        let focused = self.window_manager.focused();
                        self.execute(&command, &mut ExecutionContext::for_window(focused));
                    }
                }
            }
            other => {
                if touch::handle(self, &other, absolute_area)
                    || tablet::handle(self, &other, absolute_area)
                {
                    return;
                }
                let pointer = self
                    .seat
                    .get_pointer()
                    .expect("the seat gets a pointer at startup");
                let _ = gestures::forward(self, &pointer, &other);
            }
        }
    }

    /// Sends a key from a virtual keyboard, read with that keyboard's own
    /// keymap (or the seat's, when it sent none).
    pub(crate) fn handle_virtual_key(&mut self, event: &VirtualKeyboardKeyEvent) {
        self.note_activity();
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
            Command::Toggle(toggle) => self.execute(toggle.next(), context),
            Command::If {
                condition,
                then,
                otherwise,
            } => {
                let window = context.window.and_then(|id| self.window_manager.window(id));
                let current = self.window_manager.current_workspace();
                if condition.holds(window, current) {
                    self.execute(then, context);
                } else if let Some(otherwise) = otherwise {
                    self.execute(otherwise, context);
                }
            }
            Command::ForEach { windows, command } => {
                let current = self.window_manager.current_workspace();
                let matching: Vec<WindowId> = self
                    .window_manager
                    .windows_top_down()
                    .filter(|window| window.is_placed() && windows.matches(window, current))
                    .map(Window::id)
                    .collect();
                for window in matching {
                    self.execute(command, &mut ExecutionContext::for_window(Some(window)));
                }
            }
            Command::Delay(delay) => self.run_later(delay, context.window),
        }
    }

    /// Runs `delay`'s command for `window` once its time is up; running it
    /// again before then puts it off.
    fn run_later(&mut self, delay: &Delay, window: Option<WindowId>) {
        // Never inside a delay's own callback: that runs only its command.
        self.delays
            .retain(|(pending, timer)| !pending.is(delay) && !timer.has_fired());
        let command = delay.command.clone();
        let timer = Timer::once(
            &self.loop_handle,
            &self.ledger,
            delay.wait,
            move |compositor| {
                compositor.execute(&command, &mut ExecutionContext::for_window(window));
                compositor.schedule_render();
            },
        );
        match timer {
            Ok(timer) => self.delays.push((delay.clone(), timer)),
            Err(error) => tracing::warn!(%error, "a delayed command could not wait"),
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
        let mut target = self
            .outputs
            .at(location.to_f64())
            .map_or_else(|| self.outputs.primary_geometry(), Outputs::geometry);
        target.loc -= get_popup_toplevel_coords(&kind);
        target.loc -= location;
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    /// Returns the parts of each output windows can be placed in: all of
    /// it, and what panels, the taskbar among them, leave free.
    fn output_areas(&self) -> OutputAreas {
        let area_of = |output: &Output| {
            let whole = Outputs::geometry(output);
            let usable = OutputLayers::new(output, whole).usable_area();
            let usable = match self.taskbars.on_output(&output.name()) {
                Some(taskbar) => {
                    taskbar.reserve(usable, whole, self.config.theme.metrics().taskbar_height)
                }
                None => usable,
            };
            OutputArea { whole, usable }
        };
        OutputAreas::new(
            area_of(self.outputs.primary()),
            self.outputs.others().map(area_of).collect(),
        )
        .named(
            std::iter::once(self.outputs.primary())
                .chain(self.outputs.others())
                .map(Output::name)
                .collect(),
        )
    }

    /// Puts a taskbar on each output that has one by the settings, for the
    /// output areas about to be worked out, which then take their room;
    /// returns whether any came or went.
    fn sync_taskbars_quietly(&mut self) -> bool {
        let main = self.taskbar_output().name();
        let others: Vec<String> = self.outputs.iter().map(Output::name).collect();
        let has_changed = self
            .taskbars
            .follow_outputs(&main, others.iter().map(String::as_str));
        if has_changed {
            self.start_taskbar_services();
        }
        has_changed
    }

    /// Puts a taskbar on each output that has one by the settings, the
    /// windows kept off those that came.
    fn sync_taskbars(&mut self) {
        if self.sync_taskbars_quietly() {
            self.refresh_output_area();
        }
    }

    /// Returns the output the main taskbar shows on: the one the settings
    /// name, or the primary one.
    fn taskbar_output(&self) -> &Output {
        self.config
            .settings
            .taskbar
            .output
            .as_deref()
            .and_then(|name| self.outputs.named(name))
            .unwrap_or_else(|| self.outputs.primary())
    }

    /// Returns the connector of the output `target` names, from the one
    /// the taskbar shows on; none if there is no such head.
    fn taskbar_output_named(&self, target: &TaskbarOutput) -> Option<String> {
        let head = match target {
            TaskbarOutput::Connector(connector) => return Some(connector.clone()),
            TaskbarOutput::Head(head) => *head,
        };
        let areas = Self::bare_output_areas(&self.outputs);
        let from = areas.at(Outputs::geometry(self.taskbar_output()).loc);
        let to = areas.target(from, head)?;
        std::iter::once(self.outputs.primary())
            .chain(self.outputs.others())
            .find(|output| Outputs::geometry(output) == to.whole)
            .map(Output::name)
    }

    /// Returns each of `outputs` whole, before panels take any of it.
    fn bare_output_areas(outputs: &Outputs) -> OutputAreas {
        let area_of = |output: &Output| {
            let whole = Outputs::geometry(output);
            OutputArea {
                whole,
                usable: whole,
            }
        };
        OutputAreas::new(
            area_of(outputs.primary()),
            outputs.others().map(area_of).collect(),
        )
    }

    /// Draws the time anew, and wakes vaht when it next changes: the
    /// taskbar's clock's moment came, or the time of day jumped.
    pub(crate) fn show_time_change(&mut self) {
        self.schedule_render();
        self.schedule_clock();
    }

    /// Starts what the taskbar keeps running: its clock's alarm and its
    /// tray, as it shows them.
    fn start_taskbar_services(&mut self) {
        self.schedule_clock();
        self.taskbars.start_tray((&self.loop_handle, &self.ledger));
    }

    /// Wakes vaht when a taskbar's clock next changes, if one shows.
    pub(crate) fn schedule_clock(&mut self) {
        self.taskbars
            .schedule_clock(&Zoned::now(), (&self.loop_handle, &self.ledger));
    }

    /// Hands a wheel turn of `steps` (in 120ths of a notch, positive down)
    /// to the tray item under the pointer; returns whether one was there.
    fn scroll_tray_item(&mut self, steps: Option<f64>) -> bool {
        let location = self.pointer_location();
        let Some(PointerTarget::Taskbar(Some(TaskbarHit::TrayItem(item)))) =
            self.scene().target_at(location)
        else {
            return false;
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a wheel turn is a few notches, far inside i32"
        )]
        let notches = (steps.unwrap_or(0.0) / 120.0).round() as i32;
        if notches != 0 {
            self.taskbars
                .act_on_tray(item, TrayAction::Scroll(notches), location);
        }
        true
    }

    /// Lets go of a held taskbar button with the pointer at `location`,
    /// doing what clicking it means if the pointer is still on it.
    fn release_taskbar_button(&mut self, location: Point<f64, Logical>) {
        let Some(clicked) = self
            .taskbars
            .iter_mut()
            .find_map(|taskbar| taskbar.release(location))
        else {
            return;
        };
        tracing::debug!(?clicked, "taskbar button clicked");
        match clicked {
            TaskbarHit::WorkspacePrevious => self.window_manager.run(
                &WindowManagerAction::Workspace(WorkspaceTarget::Previous),
                None,
            ),
            TaskbarHit::WorkspaceNext => self
                .window_manager
                .run(&WindowManagerAction::Workspace(WorkspaceTarget::Next), None),
            TaskbarHit::Entry(window) => self.window_manager.activate_or_minimize(window),
            // Never held down: neither is a button, and tray items act as
            // they are pressed.
            TaskbarHit::WorkspaceName | TaskbarHit::Clock | TaskbarHit::TrayItem(_) => {}
        }
        self.schedule_render();
    }

    /// Moves the pointer to `position` on `surface`, as its client asks
    /// with the serial of the pointer's last entering one of its surfaces
    /// (`pointer-warp`); refused unless `surface` has the pointer and the
    /// position is on it.
    pub(crate) fn warp_pointer(
        &mut self,
        surface: &WlSurface,
        position: Point<f64, Logical>,
        serial: Serial,
    ) {
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let under = self
            .scene()
            .target_at(self.pointer_location())
            .and_then(|target| target.client_surface());
        let origin = under
            .filter(|(focused, _)| focused == surface)
            .map(|(_, origin)| origin);
        let size = with_renderer_surface_state(surface, |state| state.surface_size())
            .flatten()
            .unwrap_or_default();
        let entered = pointer.last_enter();
        match origin {
            Some(origin) if pointer_lock::may_warp(true, (serial, entered), position, size) => {
                self.move_pointer_to(origin + position, InputTime::now());
            }
            _ => tracing::debug!(?position, "a client's pointer warp was refused"),
        }
    }

    /// Takes a program's new connection to the EIS server.
    pub(crate) fn emulating_client_connected(&mut self, context: reis::eis::Context) {
        if let Some(emulation) = &mut self.input_emulation {
            emulation.add_client(context, (&self.loop_handle, &self.ledger));
        }
    }

    /// Follows what emulating client `id` sent on `connection`; returns
    /// what to do with its source.
    pub(crate) fn follow_emulating_client(
        &mut self,
        id: u64,
        event: Result<reis::calloop::EisRequestSourceEvent, reis::Error>,
        connection: &mut reis::request::Connection,
    ) -> calloop::PostAction {
        let display = self
            .xwayland
            .as_ref()
            .and_then(XwaylandServer::display_name);
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        let outputs = &self.outputs;
        let Some(emulation) = &mut self.input_emulation else {
            return calloop::PostAction::Remove;
        };
        let (action, emulated) = emulation.follow(
            (id, event),
            connection,
            (
                display.as_deref(),
                &self.config.settings.xwayland.input_emulation,
            ),
            || DeviceSetup {
                keymap: keyboard.keymap_file(),
                regions: outputs.iter().map(Outputs::geometry).collect(),
            },
        );
        self.carry_out_emulated(emulated);
        action
    }

    /// Carries out what emulating programs did: their input goes in as a
    /// device's does, but not behind the session lock.
    fn carry_out_emulated(&mut self, emulated: Vec<Emulated>) {
        for emulated in emulated {
            match emulated {
                Emulated::Pointer(event) => {
                    if !self.session_lock.is_locked() {
                        // Absolute motion is in global coordinates.
                        let whole = Rectangle::from_size(self.outputs.bounds().size);
                        self.handle_input_within(event, Some(whole));
                    }
                }
                Emulated::Key {
                    code,
                    is_pressed,
                    source,
                } => {
                    if !self.session_lock.is_locked() {
                        self.note_activity();
                        let state = if is_pressed {
                            KeyState::Pressed
                        } else {
                            KeyState::Released
                        };
                        self.press_key(source, Keycode::new(code + 8), state, InputTime::now());
                    }
                }
                Emulated::Release { buttons, source } => self.release_emulated(source, &buttons),
                Emulated::Ask { id, name } => {
                    tracing::info!(id, program = name, "asking about input emulation");
                }
            }
        }
        self.show_input_emulation_question();
    }

    /// Lets go of the keys of keyboard `source` and of `buttons`, which an
    /// emulating program held.
    fn release_emulated(&mut self, source: KeyboardSource, buttons: &[u32]) {
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        keyboard.release_source(self, source);
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        for button in buttons {
            pointer.button(
                self,
                &ButtonEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: InputTime::now(),
                    button: *button,
                    state: ButtonState::Released,
                },
            );
        }
        pointer.frame(self);
    }

    /// Lets go of what every emulating program holds: the session locked.
    pub(crate) fn release_all_emulated(&mut self) {
        if let Some(emulation) = &mut self.input_emulation {
            let emulated = emulation.release_all();
            self.carry_out_emulated(emulated);
        }
    }

    /// Follows the question about input emulation: one closed unanswered
    /// is a refusal, and the next waiting is shown.
    pub(crate) fn follow_input_emulation_question(&mut self) {
        let Some(emulation) = &mut self.input_emulation else {
            return;
        };
        emulation.prune();
        if let Some(asked) = emulation.prompted()
            && !self.menus.is_open()
        {
            let emulated = emulation.answer(asked, false);
            self.carry_out_emulated(emulated);
        }
        self.show_input_emulation_question();
    }

    /// Adds `program` to the programs that may (`is_allowed`) or may not
    /// emulate input, as a change of the settings in place, which lasts
    /// across reloads and which `save-settings` writes.
    fn remember_emulation_answer(&mut self, program: &str, is_allowed: bool) {
        let settings = &self.config.settings.xwayland.input_emulation;
        let (list, name) = if is_allowed {
            (&settings.allow, "allow")
        } else {
            (&settings.deny, "deny")
        };
        if list.iter().any(|known| known == program) {
            return;
        }
        let programs = list
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(program))
            .map(|known| toml::Value::String(known.to_owned()))
            .collect();
        self.change_setting(SettingChange::new(
            &["xwayland", "input_emulation", name],
            &toml::Value::Array(programs),
        ));
    }

    /// Shows the question about the next program waiting to emulate input,
    /// if no question shows.
    fn show_input_emulation_question(&mut self) {
        if self.session_lock.is_locked() {
            return;
        }
        let Some((client, program)) = self
            .input_emulation
            .as_mut()
            .and_then(InputEmulation::next_question)
        else {
            return;
        };
        tracing::info!(program, "asking whether a program may emulate input");
        let choice = |label: &str, is_allowed, is_lasting: bool| MenuItem::Choice {
            label: label.to_owned(),
            choice: MenuChoice::AnswerInputEmulation {
                client,
                is_allowed,
                from_now_on: is_lasting.then(|| program.clone()),
            },
            mark: None,
            icon: None,
        };
        let model = MenuModel {
            title: Some(format!("Let {program} control the pointer and keys?")),
            items: vec![
                choice("Allow", true, false),
                choice("Allow from now on", true, true),
                choice("Deny", false, false),
                choice("Deny from now on", false, true),
            ],
        };
        let at = self.pointer_location().to_i32_round();
        let bounds = self.output_areas().at(at).usable;
        self.menus.open(
            model,
            at,
            bounds,
            (&self.config.theme, &mut self.typesetter),
        );
        self.schedule_render();
    }

    /// Returns where the pointer is.
    pub(crate) fn pointer_location(&self) -> Point<f64, Logical> {
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
    pub(crate) fn settle(&mut self) {
        self.refresh_output_area();
        let updates = self.window_manager.take_client_updates();
        for (window, requested) in &updates.states {
            let location = self
                .window_manager
                .window(*window)
                .map(|window| window.content_area().loc);
            if let (Some(toplevel), Some(location)) = (self.toplevels.get_mut(*window), location) {
                toplevel.request(requested, location);
            }
        }
        // An X11 window is told where it is whenever it moves.
        for window in self.window_manager.windows_top_down() {
            if let Some(toplevel) = self.toplevels.get_mut(window.id()) {
                toplevel.place(window.content_area());
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
        self.layers.refresh_keyboard(LayerMaps::new(&self.outputs));
    }

    /// Tells the window manager where windows can be placed now.
    fn refresh_output_area(&mut self) {
        self.sync_taskbars_quietly();
        let outputs = self.output_areas();
        self.window_manager.set_output_areas(outputs);
    }

    /// Gives the seat's keyboard to the focused window, unless a layer
    /// surface holds it (a launcher keeps it), if that changed.
    fn sync_keyboard_focus(&mut self) {
        let window_target = self
            .window_manager
            .focused()
            .and_then(|window| self.toplevels.get(window))
            .map(Toplevel::focus_target);
        let target = if self.session_lock.is_locked() {
            // Only the locker gets keys: its primary output's surface.
            self.session_lock
                .surface_on(self.outputs.primary())
                .or_else(|| {
                    self.session_lock
                        .surfaces()
                        .next()
                        .map(|(_, surface)| surface)
                })
                .cloned()
                .map(FocusTarget::Surface)
        } else {
            self.layers
                .keyboard()
                .cloned()
                .map(FocusTarget::Surface)
                .or(window_target)
        };
        // A focused target that is gone reads as `Some(None)`, unlike any
        // target or none, so its client's focus is always cleared.
        let given = self
            .keyboard_focus
            .as_ref()
            .map(|given| given.alive().then(|| given.clone()));
        if given == target.clone().map(Some) {
            return;
        }
        if let Some(previous) = self.keyboard_focus.as_ref().and_then(FocusTarget::surface)
            && target.as_ref().and_then(FocusTarget::surface).as_ref() != Some(&previous)
        {
            self.deactivate_pointer_constraint(&previous);
        }
        self.keyboard_focus.clone_from(&target);
        // Smithay clears the focus of a destroyed surface without telling
        // SeatHandler::focus_changed, so the clipboard focus is set here too:
        // otherwise it keeps the last focused client's state alive after
        // that client disconnects.
        let client = target
            .as_ref()
            .and_then(FocusTarget::surface)
            .and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, &self.seat, client.clone());
        set_primary_focus(&self.display_handle, &self.seat, client);
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        // vaht decides where the keyboard goes: an X program's keyboard
        // grab, the only one there is, ends when focus moves on.
        if keyboard.is_grabbed() {
            keyboard.unset_grab(self);
        }
        keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
    }

    /// Returns the focus an X program's keyboard grab of its window's
    /// `surface` holds the keyboard on: the window, if it has the keyboard
    /// now; a window in the background cannot take it.
    pub(crate) fn x11_keyboard_grab_target(&self, surface: &WlSurface) -> Option<FocusTarget> {
        self.keyboard_focus
            .clone()
            .filter(|focus| matches!(focus, FocusTarget::X11(_)))
            .filter(|focus| focus.surface().as_ref() == Some(surface))
    }

    /// Updates the window whose root surface is `root` after a commit of
    /// it (`is_root`) or of one of its subsurfaces.
    fn commit_window(&mut self, root: &WlSurface, is_root: bool) {
        let Some(window) = self.toplevels.window_of(root) else {
            return;
        };
        let Some(toplevel) = self.toplevels.get_mut(window) else {
            return;
        };
        toplevel.on_commit();
        if is_root {
            self.commit_toplevel(window);
        }
    }

    /// Tells the window manager which window the window whose root
    /// surface is `root` is a dialog of, as its client says now.
    pub(crate) fn follow_family(&mut self, root: &WlSurface) {
        let Some(window) = self.toplevels.window_of(root) else {
            return;
        };
        let Some(toplevel) = self.toplevels.get(window) else {
            return;
        };
        let (parent, is_modal) = toplevel.family();
        let parent = parent.and_then(|link| self.toplevels.window_named(&link));
        self.window_manager.set_family(window, parent, is_modal);
        self.settle();
    }

    /// Tells the window manager what `window`'s client committed, and
    /// carries out what it decides.
    fn commit_toplevel(&mut self, window: WindowId) {
        let Some(toplevel) = self.toplevels.get_mut(window) else {
            return;
        };
        let (parent, is_modal) = toplevel.family();
        let backend = &mut self.backend;
        let committed = toplevel.committed_state(|buffer| backend.read_icon(buffer));
        // Before the commit, so a new dialog is placed over its parent.
        let parent = parent.and_then(|link| self.toplevels.window_named(&link));
        self.window_manager.set_family(window, parent, is_modal);
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
            Action::RunDialog => self
                .launcher
                .show_dialog("run", self.config_source.directory()),
            Action::CommandDialog => self
                .launcher
                .show_dialog("command", self.config_source.directory()),
            Action::SetTheme(name) => self.change_setting(SettingChange::theme(name)),
            Action::SetSetting(change) => self.change_setting(change.clone()),
            Action::ApplyProfile(name) => {
                match self
                    .outputs
                    .apply_profile(name, &self.config.outputs, &mut self.backend)
                {
                    Ok(changes) => self.follow_output_changes(&changes),
                    Err(message) => tracing::warn!(message, "the profile was not applied"),
                }
            }
            Action::CycleProfile => {
                if let Some(changes) = self
                    .outputs
                    .cycle_profile(&self.config.outputs, &mut self.backend)
                {
                    self.follow_output_changes(&changes);
                } else {
                    tracing::info!("no other profile fits the outputs connected");
                }
            }
            Action::CyclePrimaryOutput => {
                if let Some(changes) = self.outputs.cycle_primary(&self.config.outputs) {
                    self.follow_output_changes(&changes);
                }
            }
            Action::SetPrimaryOutput(connector) => {
                if let Some(changes) = self.outputs.choose_primary(connector) {
                    self.follow_output_changes(&changes);
                }
            }
            Action::SaveSettings => {
                if let Err(error) = self.overrides.save(&self.config_source) {
                    tracing::warn!(
                        error = &error as &dyn std::error::Error,
                        "could not save the settings"
                    );
                }
            }
            Action::SetTitleDialog => {
                if let Some(window) = context.window {
                    self.launcher.show_window_dialog(
                        "title",
                        window,
                        self.config_source.directory(),
                    );
                }
            }
            Action::Exit => self.stop(),
            Action::SetEnvironment { name, value } => {
                self.launcher.set_environment(name.clone(), value.clone());
            }
            Action::OpenMenu(request) => self.open_menu(request, context.window),
            Action::HideMenus => {
                self.menus.close();
                self.schedule_render();
            }
            Action::Taskbar(action) => self.run_taskbar_action(action),
            Action::KeyMode(switch) => self.input.switch_key_mode(switch, &self.config.bindings),
        }
    }

    /// Runs one action on the taskbar.
    fn run_taskbar_action(&mut self, action: &TaskbarAction) {
        match action {
            TaskbarAction::Toggle => {
                if self.taskbars.is_enabled() {
                    self.taskbars.toggle();
                    // Their room is given back or taken, and their clocks
                    // rest while they are off.
                    self.refresh_output_area();
                    self.schedule_clock();
                    self.schedule_render();
                }
            }
            TaskbarAction::ActivateEntry(number) => {
                // The taskbar on the output the pointer is on lists the
                // entries counted; the main one otherwise.
                let pointer_output = self.outputs.at(self.pointer_location()).map(Output::name);
                let taskbar = pointer_output
                    .and_then(|name| self.taskbars.on_output(&name))
                    .or_else(|| self.taskbars.main());
                let window = taskbar.and_then(|taskbar| {
                    let whole = self
                        .outputs
                        .named(taskbar.output())
                        .map_or_else(|| self.outputs.primary_geometry(), Outputs::geometry);
                    taskbar.entry(
                        &self.window_manager,
                        &self.config.settings.taskbar,
                        (*number, whole),
                    )
                });
                if let Some(window) = window {
                    self.window_manager.activate_or_minimize(window);
                    self.schedule_render();
                }
            }
            TaskbarAction::TrayItem { number, request } => {
                if let Some((item, location)) = self.taskbars.tray_item(*number) {
                    self.taskbars.act_on_tray(item, request.action(), location);
                    self.tray_grant = self
                        .taskbars
                        .tray_item_pid(item)
                        .map(|pid| (pid, Instant::now()));
                }
            }
            TaskbarAction::ToOutput(target) => {
                if let Some(connector) = self.taskbar_output_named(target) {
                    self.change_setting(SettingChange::new(
                        &["taskbar", "output"],
                        &toml::Value::String(connector),
                    ));
                }
            }
        }
    }

    /// Opens the menu `request` names, its corner at the pointer, about
    /// `window` (the window menu's; the focused window when none); asked
    /// again while it is open, as fluxland does, it closes.
    fn open_menu(&mut self, request: &MenuRequest, window: Option<WindowId>) {
        if self.menus.is_opened_by(request) {
            self.menus.close();
            self.schedule_render();
            return;
        }
        let window = window.or_else(|| self.window_manager.focused());
        let model = self.with_menu_builder(window, &[], |builder| match request {
            MenuRequest::Root => builder.build(Menus::ROOT),
            MenuRequest::Window => window.and_then(|_| builder.build(Menus::WINDOW)),
            MenuRequest::Workspace => builder.build(Menus::WORKSPACE),
            MenuRequest::WindowList => Some(builder.generated(MenuGenerator::WindowList, None)),
            MenuRequest::Custom(name) => builder.build(name),
        });
        let Some(model) = model else {
            tracing::warn!(?request, "no such menu, or no window for it");
            return;
        };
        let at = self.pointer_location().to_i32_round();
        // Off the taskbar, as fluxland keeps menus in the usable area of
        // the output the pointer is on.
        let bounds = self.output_areas().at(at).usable;
        self.menus.open(
            model,
            at,
            bounds,
            (&self.config.theme, &mut self.typesetter),
        );
        self.menus.mark_opened_by(request.clone(), window);
        self.schedule_render();
    }

    /// Runs `build` with a builder of menus about `window`, as the
    /// configuration and vaht's state make them, with the icons
    /// `menus.toml` and `extra_icons` name found.
    fn with_menu_builder<T>(
        &mut self,
        window: Option<WindowId>,
        extra_icons: &[&str],
        build: impl FnOnce(&MenuBuilder<'_>) -> T,
    ) -> T {
        let icons = self.window_icons_for_menus();
        let named_icons = self.named_icons_for_menus(extra_icons);
        let builder = MenuBuilder::new(&self.config.menus, &self.window_manager, window)
            .with_named_icons(named_icons)
            .with_themes(
                ThemeFile::available(self.config_source.directory()),
                self.config.settings.theme.as_deref(),
            )
            .with_window_icons(icons)
            .with_outputs(OutputChoices {
                profiles: self
                    .config
                    .outputs
                    .profile_names()
                    .map(str::to_owned)
                    .collect(),
                profile: self.outputs.profile().map(str::to_owned),
                outputs: self.outputs.iter().map(Output::name).collect(),
            });
        build(&builder)
    }

    /// Runs the command of the pipe menu just opened, if one did, reading
    /// its output as it comes.
    fn start_pipe_menu(&mut self) {
        let Some((number, command)) = self.menus.take_pipe_command() else {
            return;
        };
        tracing::debug!(command, "pipe menu opened");
        let started = self
            .launcher
            .run_reading(&command)
            .map_err(|error| error.to_string())
            .and_then(|reader| {
                PipeRead::start(number, reader, (&self.loop_handle, &self.ledger))
                    .map_err(|error| error.to_string())
            });
        match started {
            Ok(read) => self.menus.read_pipe(read),
            Err(error) => {
                tracing::warn!(error, command, "a pipe menu's command could not start");
                self.fill_pipe_menu(number, Err(error));
            }
        }
    }

    /// Fills pipe menu `number` with what its command printed, now that it
    /// ended.
    pub(crate) fn finish_pipe_menu(&mut self, number: u64) {
        if let Some(output) = self.menus.take_pipe_output(number) {
            self.fill_pipe_menu(number, output);
        }
    }

    /// Fills pipe menu `number` with the menu `output` holds, or says why
    /// there is none.
    fn fill_pipe_menu(&mut self, number: u64, output: Result<Vec<u8>, String>) {
        let printed = output.and_then(|output| parse_printed(&output));
        let model = match printed {
            Ok(definition) => {
                let icons: Vec<String> = definition
                    .items
                    .iter()
                    .filter_map(MenuEntry::icon)
                    .map(str::to_owned)
                    .collect();
                let icons: Vec<&str> = icons.iter().map(String::as_str).collect();
                let window = self.menus.opened_for();
                self.with_menu_builder(window, &icons, |builder| builder.build_printed(&definition))
            }
            Err(error) => {
                tracing::warn!(error, "a pipe menu's command printed no menu");
                MenuModel {
                    title: None,
                    items: vec![MenuItem::Inert {
                        label: "(no menu)".to_owned(),
                    }],
                }
            }
        };
        if self
            .menus
            .fill_pipe(number, model, (&self.config.theme, &mut self.typesetter))
        {
            tracing::debug!(number, "pipe menu filled");
            self.schedule_render();
        }
    }

    /// Returns each placed window's icon at the size a menu item shows it;
    /// none if the settings want no icons in menus.
    fn window_icons_for_menus(&mut self) -> std::collections::HashMap<WindowId, vaht_ui::Icon> {
        if !self.config.settings.menus.window_icons {
            return std::collections::HashMap::new();
        }
        let size = vaht_ui::WidgetIcon::side(self.config.theme.metrics().menu.item_height);
        self.window_manager
            .windows_top_down()
            .filter_map(|window| Some((window.id(), window.icon_in(&mut self.icons, size)?)))
            .collect()
    }

    /// Returns the icons `menus.toml` names, by the name written, those
    /// found, sized for menu items.
    fn named_icons_for_menus(
        &mut self,
        extra: &[&str],
    ) -> std::collections::HashMap<String, vaht_ui::Icon> {
        let size = vaht_ui::WidgetIcon::side(self.config.theme.metrics().menu.item_height);
        let directory = self.config_source.directory();
        self.config
            .menus
            .icons()
            .into_iter()
            .chain(extra.iter().copied())
            .filter_map(|icon| {
                let lookup = Menus::icon_lookup(icon, directory)?;
                Some((icon.to_owned(), self.icons.named(&lookup, size)?))
            })
            .collect()
    }

    /// Opens the submenu of the item highlighted in the last menu: the
    /// pointer has rested on it.
    pub(crate) fn open_highlighted_submenu(&mut self) {
        self.menus
            .open_highlighted_submenu((&self.config.theme, &mut self.typesetter));
        self.start_pipe_menu();
        self.schedule_render();
    }

    /// Does what the menu item chosen does.
    fn choose_from_menu(&mut self, choice: MenuChoice) {
        tracing::debug!(?choice, "menu item chosen");
        match choice {
            MenuChoice::Run { command, window } => {
                self.execute(&command, &mut ExecutionContext::for_window(window));
            }
            MenuChoice::Activate(window) => self.window_manager.activate(window),
            MenuChoice::AnswerInputEmulation {
                client,
                is_allowed,
                from_now_on,
            } => {
                if let Some(emulation) = &mut self.input_emulation {
                    let emulated = emulation.answer(client, is_allowed);
                    self.carry_out_emulated(emulated);
                }
                if let Some(program) = from_now_on {
                    self.remember_emulation_answer(&program, is_allowed);
                }
            }
        }
        self.schedule_render();
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
        match bound {
            Some(Some(KeyPress::Binding(command))) => self.run_key_binding(keycode, command),
            Some(Some(KeyPress::Menu(key))) => {
                let chosen = self
                    .menus
                    .key(key, (&self.config.theme, &mut self.typesetter));
                self.start_pipe_menu();
                self.schedule_render();
                if let Some(choice) = chosen {
                    self.choose_from_menu(choice);
                }
            }
            Some(Some(KeyPress::SwitchTerminal(terminal))) => {
                if let Backend::Drm(backend) = &mut self.backend {
                    backend.switch_terminal(terminal);
                }
            }
            Some(None) | None => {}
        }
    }

    /// Decides whether a key event is vaht's: a bound press, or the release
    /// of one, or any key while a menu is open. Returns what the press does.
    fn filter_key(
        &mut self,
        keycode: Keycode,
        state: KeyState,
        modifiers: &ModifiersState,
        keysym: &KeysymHandle<'_>,
    ) -> FilterResult<Option<KeyPress>> {
        let held = Modifiers::held(modifiers);
        if state == KeyState::Released && held == Modifiers::default() {
            self.window_manager.modifiers_released();
        }
        // Switching terminals comes before everything, so a stuck menu or
        // binding never traps the user on vaht's terminal.
        if state == KeyState::Pressed
            && matches!(self.backend, Backend::Drm(_))
            && let Some(terminal) = virtual_terminal(keysym.modified_sym())
        {
            self.input.claim_release(keycode);
            return FilterResult::Intercept(Some(KeyPress::SwitchTerminal(terminal)));
        }
        // A window taking every key gets them, bound or not; a locked
        // session's keys are its locker's.
        if self.are_shortcuts_inhibited() || self.session_lock.is_locked() {
            return FilterResult::Forward;
        }
        if self.menus.is_open() && state == KeyState::Pressed {
            // Keys are the menus' while one is open, their releases too.
            self.input.claim_release(keycode);
            return FilterResult::Intercept(Some(KeyPress::Menu(MenuKey::of(
                keysym.modified_sym(),
            ))));
        }
        let chord = keysym
            .raw_latin_sym_or_raw_current_sym()
            .map(|key| KeyChord::new(held, key));
        let press = (state, Instant::now());
        match self.input.key(keycode, press, chord, &self.config.bindings) {
            FilterResult::Forward => FilterResult::Forward,
            FilterResult::Intercept(command) => {
                FilterResult::Intercept(command.map(KeyPress::Binding))
            }
        }
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

    /// Notes what has a tooltip under the pointer at `location`: the
    /// taskbar's entries and tray items, while no menu is open.
    fn follow_tooltip(&mut self, location: Point<f64, Logical>) {
        let text = self
            .taskbars
            .at(location)
            .filter(|_| !self.menus.is_open())
            .and_then(|taskbar| {
                taskbar.tooltip_at(location, &self.config.theme, &mut self.typesetter)
            });
        let delay = self.config.settings.taskbar.tooltip_delay();
        if self
            .tooltip
            .follow(text, delay, (&self.loop_handle, &self.ledger))
        {
            self.schedule_render();
        }
    }

    /// Shows the tooltip for what the pointer has rested on.
    pub(crate) fn show_tooltip(&mut self) {
        let pointer = self.pointer_location();
        let bounds = self
            .outputs
            .at(pointer)
            .map_or_else(|| self.outputs.primary_geometry(), Outputs::geometry);
        let pointer = pointer.to_i32_round();
        self.tooltip
            .show(pointer, bounds, (&self.config.theme, &mut self.typesetter));
        tracing::debug!("tooltip shown");
        self.schedule_render();
    }

    /// Moves the pointer to `location`, telling the surface under it.
    pub(crate) fn move_pointer_to(&mut self, location: Point<f64, Logical>, time: InputTime) {
        // With strict sloppy focus, what was under the pointer where it
        // was, as the windows are stacked now: moving within one window
        // never moves focus, however it came there.
        let was_over = (self.config.settings.focus.model == FocusModel::StrictSloppy)
            .then(|| {
                let from = self.pointer_location();
                self.scene()
                    .target_at(from)
                    .as_ref()
                    .and_then(PointerTarget::window)
            })
            .flatten();
        let outputs = &self.outputs;
        let mut has_shown = false;
        for taskbar in self.taskbars.iter_mut() {
            let whole = outputs
                .named(taskbar.output())
                .map_or_else(|| outputs.primary_geometry(), Outputs::geometry);
            has_shown |= taskbar.follow_pointer(location, whole, (&self.loop_handle, &self.ledger));
        }
        if has_shown {
            // It showed: its clock ticks again, and the output shows it.
            self.schedule_clock();
            self.schedule_render();
        }
        self.follow_tooltip(location);
        if self.menus.is_open()
            && self.menus.follow_pointer(
                location,
                self.config.settings.menus.submenu_delay(),
                (&self.loop_handle, &self.ledger),
            )
        {
            self.schedule_render();
        }
        // Over a menu, no client has the pointer.
        let target = if self.menus.covers(location) {
            None
        } else {
            self.scene().target_at(location)
        };
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        if let Some(window) = target.as_ref().and_then(PointerTarget::window)
            && !pointer.is_grabbed()
            && was_over != Some(window)
        {
            self.pointer_entered(window);
        }
        // Over a frame, no client has the pointer: the frame is vaht's.
        let under = target.as_ref().and_then(PointerTarget::client_surface);
        let moved_from = pointer.current_location();
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
        // Where vaht draws the pointer itself (on the hardware), it shows
        // where it went only in a new frame; elsewhere the host draws it.
        if self.backend.draws_pointer() && moved_from != location {
            self.schedule_render();
        }
    }

    /// With sloppy focus, focuses `window`, which the pointer is over, and
    /// with auto-raise raises it once the pointer has rested there a while.
    fn pointer_entered(&mut self, window: WindowId) {
        let focus = self.config.settings.focus;
        if !focus.model.follows_pointer() || self.window_manager.focused() == Some(window) {
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
        self.pointer_button(event.button_code(), event.state(), event.time());
    }

    /// Presses or releases the pointer's `button` (`state`) at `time`: a
    /// mouse's, or a touch on vaht's own parts standing in for one.
    pub(crate) fn pointer_button(&mut self, button: u32, state: ButtonState, time: InputTime) {
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let serial = SERIAL_COUNTER.next_serial();
        let location = pointer.current_location();
        match state {
            ButtonState::Pressed if !pointer.is_grabbed() => {
                if self.tooltip.hide() {
                    self.schedule_render();
                }
                self.press_button(button, location, serial);
            }
            ButtonState::Pressed => {}
            ButtonState::Released => {
                self.window_manager.release_titlebar_button(location);
                self.release_taskbar_button(location);
                if self.menus.is_open() {
                    let chosen = self
                        .menus
                        .release(location, (&self.config.theme, &mut self.typesetter));
                    self.start_pipe_menu();
                    self.schedule_render();
                    if let Some(choice) = chosen {
                        self.choose_from_menu(choice);
                    }
                }
            }
        }
        // A client clicked has keyboard focus by the time it hears of it.
        self.settle();
        pointer.button(
            self,
            &ButtonEvent {
                button,
                state,
                serial,
                time,
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
        if self.menus.is_open() {
            // A press on a menu is the menu's; elsewhere it closes the
            // menus and goes on to what is there.
            let is_on_menu = self.menus.press(location);
            self.schedule_render();
            if is_on_menu {
                return;
            }
        }
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
            PressOutcome::ClickUnmanaged => return,
            PressOutcome::ClickTaskbar { window, then } => (window, then),
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
            AfterClick::HoldTaskbarButton => {
                if let Some(taskbar) = self.taskbars.at_mut(location) {
                    taskbar.press(location);
                }
                self.schedule_render();
            }
            AfterClick::AskTrayItem(item, action) => {
                self.taskbars.act_on_tray(item, action, location);
                self.tray_grant = self
                    .taskbars
                    .tray_item_pid(item)
                    .map(|pid| (pid, Instant::now()));
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
        if self.scroll_tray_item(event.amount_v120(Axis::Vertical))
            || self.run_scroll_binding(event.amount_v120(Axis::Vertical))
        {
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
        let window = target.as_ref().and_then(PointerTarget::binding_window);
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
    /// vaht's own program, which shows its dialogs (`vaht dialog run`).
    pub executable: PathBuf,
    /// Whether Xwayland may start, if the settings want it; off for runs
    /// that need no X11 programs, such as most tests.
    pub allows_xwayland: bool,
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

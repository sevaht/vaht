//! A Wayland client that opens windows and layer surfaces, drives a virtual
//! pointer and keyboard, and takes screenshots, all on one connection.
//!
//! Requests on one connection reach vaht in order, so a screenshot taken
//! after pointer input shows that input's effect. [`TestClient`] is what
//! tests use; its event handling is in `state`, its virtual devices in
//! `input`.

mod input;
mod state;

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm};
use smithay_client_toolkit::reexports::protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1::{self, ZwpPointerConstraintsV1};
use smithay_client_toolkit::reexports::protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;
use smithay_client_toolkit::reexports::protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use smithay_client_toolkit::reexports::protocols::ext::session_lock::v1::client::ext_session_lock_v1::ExtSessionLockV1;
use smithay_client_toolkit::reexports::protocols::wp::idle_inhibit::zv1::client::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_v1::ZwpRelativePointerV1;
use smithay_client_toolkit::reexports::client::{Connection, EventQueue};
use smithay_client_toolkit::reexports::protocols::wp::presentation_time::client::wp_presentation::WpPresentation;
use smithay_client_toolkit::reexports::protocols::wp::fifo::v1::client::wp_fifo_manager_v1::WpFifoManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::fifo::v1::client::wp_fifo_v1::WpFifoV1;
use smithay_client_toolkit::reexports::protocols::wp::commit_timing::v1::client::wp_commit_timing_manager_v1::WpCommitTimingManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::commit_timing::v1::client::wp_commit_timer_v1::WpCommitTimerV1;
use smithay_client_toolkit::reexports::protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use smithay_client_toolkit::reexports::protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::Options;
use smithay_client_toolkit::shell::WaylandSurface as _;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerSurface,
};
use smithay_client_toolkit::shell::xdg::window::WindowDecorations;

pub use self::input::keys;
use self::input::{KeyMotion, VirtualInput};
use self::state::{
    CaptureFrameEvents, CaptureSessionEvents, ClientState, FeedbackEvents, IdleEvents,
    KeyboardEvents, LockEvents, LockSurfaceEvents, NoEvents, PresentationEvents,
    RelativeMotionEvents, SessionLockEvents, bind,
};
use crate::screenshot::Screenshot;

/// A layer of the layer shell, for [`TestClient::open_bar`] and
/// [`TestClient::open_backdrop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestLayer {
    /// Below everything: wallpapers.
    Background,
    /// Below the windows.
    Bottom,
    /// Above the windows: panels.
    Top,
    /// Above everything: notifications and launchers.
    Overlay,
}

impl From<TestLayer> for Layer {
    fn from(layer: TestLayer) -> Self {
        match layer {
            TestLayer::Background => Self::Background,
            TestLayer::Bottom => Self::Bottom,
            TestLayer::Top => Self::Top,
            TestLayer::Overlay => Self::Overlay,
        }
    }
}

/// The size of every test window, in pixels: large enough for a titlebar
/// with its title and buttons.
pub const WINDOW_SIZE: (u32, u32) = (240, 120);

/// The color test windows fill themselves with: unlike anything vaht draws,
/// so screenshots can find the windows.
pub const WINDOW_COLOR: [u8; 3] = [0x20, 0x60, 0xc0];

/// The Linux input code of the left mouse button (`BTN_LEFT`).
pub const BUTTON_LEFT: u32 = 0x110;

/// The Linux input code of the right mouse button (`BTN_RIGHT`).
pub const BUTTON_RIGHT: u32 = 0x111;

/// Bytes per pixel of an ARGB8888 buffer.
const BYTES_PER_PIXEL: u32 = 4;

/// What has the client's keyboard focus, as vaht last told it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardFocus {
    /// None of its surfaces.
    Nothing,
    /// Its window opened at this position, from 0.
    Window(usize),
    /// Its layer surface opened at this position, from 0.
    Layer(usize),
}

/// A client connected to vaht.
#[derive(Debug)]
pub struct TestClient {
    /// The client's event queue.
    queue: EventQueue<ClientState>,
    /// What the client knows and has.
    state: ClientState,
    /// The virtual pointer and keyboard it drives vaht with.
    input: VirtualInput,
    /// The seat's keyboard, which hears where keyboard focus is.
    _seat_keyboard: wl_keyboard::WlKeyboard,
    /// The seat's pointer, which constraints are made for.
    seat_pointer: wl_pointer::WlPointer,
    /// Hears how the mouse moves.
    _relative_pointer: ZwpRelativePointerV1,
    /// Locks the pointer.
    constraints: ZwpPointerConstraintsV1,
    /// The seat.
    seat: wl_seat::WlSeat,
    /// Asks for every key.
    shortcut_inhibitors: ZwpKeyboardShortcutsInhibitManagerV1,
    /// Hears when the session goes idle.
    idle_notifier: ExtIdleNotifierV1,
    /// Keeps the session awake.
    idle_inhibitors: ZwpIdleInhibitManagerV1,
    /// Locks the session.
    session_locks: ExtSessionLockManagerV1,
    /// The session lock the client holds, if it does.
    session_lock: Option<ExtSessionLockV1>,
    /// Says when frames showed.
    presentation: WpPresentation,
    /// Paces frames (fifo).
    fifo_manager: WpFifoManagerV1,
    /// Times frames (commit timing).
    commit_timing: WpCommitTimingManagerV1,
    /// Each window's fifo and commit timer, by window, once asked for.
    pacing: Vec<(usize, WpFifoV1, WpCommitTimerV1)>,
}

/// How vaht reported a frame (`wp_presentation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    /// It showed.
    Shown {
        /// The clock `time` is by, as `clock_gettime` numbers clocks.
        clock: u32,
        /// When it showed.
        time: Duration,
        /// The time between the output's refreshes.
        refresh: Duration,
        /// Which of the output's refreshes showed it.
        sequence: u64,
    },
    /// It never showed: a later frame replaced it.
    Discarded,
}

impl TestClient {
    /// Connects to vaht's socket.
    ///
    /// # Panics
    ///
    /// Panics if the connection fails or vaht lacks a protocol the client
    /// needs.
    #[must_use]
    pub fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).expect("vaht accepts connections");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) =
            registry_queue_init(&connection).expect("vaht lists its globals");
        let handle = queue.handle();
        let seat: wl_seat::WlSeat = bind(&globals, &handle, "wl_seat");
        let input = VirtualInput::bind(&globals, &handle, &seat);
        let seat_keyboard = seat.get_keyboard(&handle, KeyboardEvents);
        let seat_pointer = seat.get_pointer(&handle, NoEvents);
        let relative_pointers: ZwpRelativePointerManagerV1 =
            bind(&globals, &handle, "zwp_relative_pointer_manager_v1");
        let relative_pointer =
            relative_pointers.get_relative_pointer(&seat_pointer, &handle, RelativeMotionEvents);
        let constraints: ZwpPointerConstraintsV1 =
            bind(&globals, &handle, "zwp_pointer_constraints_v1");
        let shortcut_inhibitors: ZwpKeyboardShortcutsInhibitManagerV1 = bind(
            &globals,
            &handle,
            "zwp_keyboard_shortcuts_inhibit_manager_v1",
        );
        let idle_notifier: ExtIdleNotifierV1 = bind(&globals, &handle, "ext_idle_notifier_v1");
        let idle_inhibitors: ZwpIdleInhibitManagerV1 =
            bind(&globals, &handle, "zwp_idle_inhibit_manager_v1");
        let session_locks: ExtSessionLockManagerV1 =
            bind(&globals, &handle, "ext_session_lock_manager_v1");
        let presentation: WpPresentation = globals
            .bind(&handle, 1..=1, PresentationEvents)
            .expect("vaht offers wp_presentation");
        let fifo_manager: WpFifoManagerV1 = bind(&globals, &handle, "wp_fifo_manager_v1");
        let commit_timing: WpCommitTimingManagerV1 =
            bind(&globals, &handle, "wp_commit_timing_manager_v1");
        let mut state = ClientState::bind(&globals, &handle);
        // Learn the outputs before anything needs them.
        queue
            .roundtrip(&mut state)
            .expect("the connection to vaht stays up");
        Self {
            queue,
            state,
            input,
            _seat_keyboard: seat_keyboard,
            seat_pointer,
            _relative_pointer: relative_pointer,
            constraints,
            seat,
            shortcut_inhibitors,
            idle_notifier,
            idle_inhibitors,
            session_locks,
            session_lock: None,
            presentation,
            fifo_manager,
            commit_timing,
            pacing: Vec::new(),
        }
    }

    /// Redraws window `index` once for each of `colors`, as fast as it
    /// can, each frame waiting for the last to show (fifo, as Vulkan's
    /// FIFO present mode does); returns how vaht reported the last frame.
    ///
    /// # Panics
    ///
    /// Panics if there is no such window, or the connection to vaht fails.
    pub fn redraw_paced(&mut self, index: usize, colors: &[[u8; 3]]) -> Presentation {
        let handle = self.queue.handle();
        let surface = self.state.windows[index].wl_surface().clone();
        let (fifo, _) = self.pacing_of(index);
        self.state.presentation = None;
        for (number, color) in colors.iter().enumerate() {
            fifo.wait_barrier();
            fifo.set_barrier();
            if number + 1 == colors.len() {
                let _feedback = self
                    .presentation
                    .feedback(&surface, &handle, FeedbackEvents);
            }
            self.state.fill(&surface, &handle, WINDOW_SIZE, *color);
        }
        self.wait_for_presentation()
    }

    /// Redraws window `index` all `color`, to show no sooner than `at` (by
    /// the monotonic clock: commit timing); returns how vaht reported the
    /// frame.
    ///
    /// # Panics
    ///
    /// Panics if there is no such window, or the connection to vaht fails.
    pub fn redraw_at(&mut self, index: usize, color: [u8; 3], at: Duration) -> Presentation {
        let handle = self.queue.handle();
        let surface = self.state.windows[index].wl_surface().clone();
        let (_, timer) = self.pacing_of(index);
        let seconds = at.as_secs();
        timer.set_timestamp(
            u32::try_from(seconds >> 32).expect("a 64-bit time's high half"),
            u32::try_from(seconds & u64::from(u32::MAX)).expect("a 64-bit time's low half"),
            at.subsec_nanos(),
        );
        self.state.presentation = None;
        let _feedback = self
            .presentation
            .feedback(&surface, &handle, FeedbackEvents);
        self.state.fill(&surface, &handle, WINDOW_SIZE, color);
        self.wait_for_presentation()
    }

    /// Waits for vaht to report the frame asked about, and returns how.
    fn wait_for_presentation(&mut self) -> Presentation {
        let is_reported =
            self.dispatch_for(crate::wait::PATIENCE, |state| state.presentation.is_some());
        assert!(is_reported, "vaht never reported the frame");
        self.state.presentation.expect("vaht reported the frame")
    }

    /// Returns window `index`'s fifo and commit timer, made the first time
    /// (a surface may have one of each).
    fn pacing_of(&mut self, index: usize) -> (WpFifoV1, WpCommitTimerV1) {
        if let Some((_, fifo, timer)) = self.pacing.iter().find(|(known, ..)| *known == index) {
            return (fifo.clone(), timer.clone());
        }
        let handle = self.queue.handle();
        let surface = self.state.windows[index].wl_surface().clone();
        let fifo = self.fifo_manager.get_fifo(&surface, &handle, NoEvents);
        let timer = self.commit_timing.get_timer(&surface, &handle, NoEvents);
        self.pacing.push((index, fifo.clone(), timer.clone()));
        (fifo, timer)
    }

    /// Redraws window `index` (in opening order) all `color`, and returns
    /// how vaht reported the frame that showed it (`wp_presentation`).
    ///
    /// # Panics
    ///
    /// Panics if there is no such window, or the connection to vaht fails.
    pub fn redraw_reported(&mut self, index: usize, color: [u8; 3]) -> Presentation {
        let handle = self.queue.handle();
        let surface = self.state.windows[index].wl_surface().clone();
        self.state.presentation = None;
        let _feedback = self
            .presentation
            .feedback(&surface, &handle, FeedbackEvents);
        self.state.fill(&surface, &handle, WINDOW_SIZE, color);
        self.wait_for_presentation()
    }

    /// Opens `count` windows and waits until each has been shown once.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn open_windows(&mut self, count: usize) {
        let handle = self.queue.handle();
        for number in 0..count {
            let surface = self.state.compositor.create_surface(&handle);
            let window = self.state.xdg_shell.create_window(
                surface,
                WindowDecorations::RequestServer,
                &handle,
            );
            window.set_title(format!("test window {}", number + 1));
            window.set_app_id("vaht.test");
            window.commit();
            self.state.windows.push(window);
        }
        // Each window draws when it is first configured. A window on a hidden
        // workspace gets no frame callbacks, so waiting for those would
        // wait forever; once vaht has the commits, screenshots show them.
        let expected = self.state.first_draws + count;
        self.dispatch_until(|state| state.first_draws >= expected);
        self.roundtrip();
    }

    /// Redraws window `index` (in opening order) fully transparent, and
    /// waits until vaht has the commit: what is beneath it shows through.
    ///
    /// # Panics
    ///
    /// Panics if there is no such window, or the connection to vaht fails.
    pub fn clear_window(&mut self, index: usize) {
        let handle = self.queue.handle();
        let surface = self.state.windows[index].wl_surface().clone();
        self.state
            .fill_pixels(&surface, &handle, WINDOW_SIZE, [0; 4]);
        self.roundtrip();
    }

    /// Has every window name `icon` as its icon (xdg-toplevel-icon), and
    /// waits until vaht has the commits.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn name_window_icons(&mut self, icon: &str) {
        let handle = self.queue.handle();
        for window in &self.state.windows {
            let named = self.state.toplevel_icons.create_icon(&handle, NoEvents);
            named.set_name(icon.to_owned());
            self.state
                .toplevel_icons
                .set_icon(window.xdg_toplevel(), Some(&named));
            named.destroy();
            window.commit();
        }
        self.roundtrip();
    }

    /// Has every window draw its own icon, `size` pixels square and all
    /// `color` (xdg-toplevel-icon), and waits until vaht has the commits.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn draw_window_icons(&mut self, size: u32, color: [u8; 3]) {
        let handle = self.queue.handle();
        for index in 0..self.state.windows.len() {
            let buffer = self.state.icon_buffer(size, color);
            let drawn = self.state.toplevel_icons.create_icon(&handle, NoEvents);
            drawn.add_buffer(&buffer, 1);
            let window = &self.state.windows[index];
            self.state
                .toplevel_icons
                .set_icon(window.xdg_toplevel(), Some(&drawn));
            drawn.destroy();
            window.commit();
        }
        self.roundtrip();
    }

    /// Closes every window and waits until vaht has processed it.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn close_windows(&mut self) {
        self.state.windows.clear();
        self.roundtrip();
    }

    /// Returns how many times vaht asked this client to close a window.
    #[must_use]
    pub const fn close_requests(&self) -> usize {
        self.state.close_requests
    }

    /// Returns how many of this client's layer surfaces vaht has closed,
    /// after reading everything vaht sent.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn closed_layers(&mut self) -> usize {
        self.roundtrip();
        self.state.closed_layers
    }

    /// Moves the pointer by `dx`, `dy`, as a mouse does.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn move_pointer_by(&mut self, dx: f64, dy: f64) {
        self.input.move_pointer_by((dx, dy));
        self.roundtrip();
    }

    /// Asks vaht to lock the pointer while it is over the first window,
    /// as a game does, and returns whether vaht locked it.
    ///
    /// # Panics
    ///
    /// Panics if the client has no window or the connection fails.
    pub fn lock_pointer(&mut self) -> bool {
        let handle = self.queue.handle();
        let surface = self.state.windows[0].wl_surface().clone();
        let _lock = self.constraints.lock_pointer(
            &surface,
            &self.seat_pointer,
            None,
            zwp_pointer_constraints_v1::Lifetime::Persistent,
            &handle,
            LockEvents,
        );
        self.roundtrip();
        self.roundtrip();
        self.state.is_pointer_locked
    }

    /// Asks vaht for every key while the first window has the keyboard,
    /// bound or not, as a game or virtual machine does.
    ///
    /// # Panics
    ///
    /// Panics if the client has no window or the connection fails.
    pub fn inhibit_shortcuts(&mut self) {
        let handle = self.queue.handle();
        let surface = self.state.windows[0].wl_surface().clone();
        let _inhibitor = self
            .shortcut_inhibitors
            .inhibit_shortcuts(&surface, &self.seat, &handle, NoEvents);
        self.roundtrip();
    }

    /// Asks to hear when the session has been idle `timeout` long.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn watch_idle(&mut self, timeout: Duration) {
        let handle = self.queue.handle();
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        let _notification = self
            .idle_notifier
            .get_idle_notification(millis, &self.seat, &handle, IdleEvents);
        self.roundtrip();
    }

    /// Asks that the session not go idle while the first window shows, as
    /// a video player does.
    ///
    /// # Panics
    ///
    /// Panics if the client has no window or the connection fails.
    pub fn inhibit_idle(&mut self) {
        let handle = self.queue.handle();
        let surface = self.state.windows[0].wl_surface().clone();
        let _inhibitor = self
            .idle_inhibitors
            .create_inhibitor(&surface, &handle, NoEvents);
        self.roundtrip();
    }

    /// Locks the session, as a screen locker does: a surface filled with
    /// `color` on every output. Returns whether vaht said it is locked.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn lock_session(&mut self, color: [u8; 3]) -> bool {
        let handle = self.queue.handle();
        let lock = self.session_locks.lock(&handle, SessionLockEvents);
        let outputs: Vec<wl_output::WlOutput> = self.state.outputs.outputs().collect();
        for output in &outputs {
            let surface = self.state.compositor.create_surface(&handle);
            let _lock_surface = lock.get_lock_surface(
                &surface,
                output,
                &handle,
                LockSurfaceEvents {
                    surface: surface.clone(),
                    color,
                },
            );
        }
        self.session_lock = Some(lock);
        self.dispatch_until(|state| state.is_session_locked.is_some());
        self.state.is_session_locked == Some(true)
    }

    /// Unlocks the session the client locked.
    ///
    /// # Panics
    ///
    /// Panics if the client holds no lock or the connection fails.
    pub fn unlock_session(&mut self) {
        let lock = self.session_lock.take().expect("the client holds the lock");
        lock.unlock_and_destroy();
        self.state.is_session_locked = None;
        self.roundtrip();
    }

    /// Waits up to `patience` for vaht to say the session is idle; returns
    /// whether it did.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn wait_for_idle(&mut self, patience: Duration) -> bool {
        self.dispatch_for(patience, |state| state.is_idle == Some(true))
    }

    /// Returns the keys pressed while the client had the keyboard, as Linux
    /// key codes, in order.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn received_keys(&mut self) -> Vec<u32> {
        self.roundtrip();
        self.state.received_keys.clone()
    }

    /// Returns whether vaht holds the pointer locked for the client now.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn is_pointer_locked(&mut self) -> bool {
        self.roundtrip();
        self.state.is_pointer_locked
    }

    /// Returns how far the mouse moved in all, as relative motion told the
    /// client.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn relative_motion(&mut self) -> (f64, f64) {
        self.roundtrip();
        self.state.relative_motion
    }

    /// Moves the pointer to `x`, `y` in global coordinates, which start at
    /// the first output's top left corner.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn move_pointer_to(&mut self, x: u32, y: u32) {
        let layout_size = self.layout_size();
        self.input.move_pointer_to(x, y, layout_size);
        self.roundtrip();
    }

    /// Opens a bar `thickness` pixels tall across the bottom of the output,
    /// in `layer`, filled with `color`; with `is_exclusive`, windows are not
    /// maximized over it. Waits until vaht has its first frame.
    ///
    /// # Panics
    ///
    /// Panics if vaht has no layer shell or the connection fails.
    pub fn open_bar(
        &mut self,
        layer: TestLayer,
        thickness: u32,
        is_exclusive: bool,
        color: [u8; 3],
    ) {
        let surface = self.open_layer(layer, color, |bar| {
            bar.set_anchor(Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
            bar.set_size(0, thickness);
            if is_exclusive {
                bar.set_exclusive_zone(i32::try_from(thickness).expect("bars are thin"));
            }
        });
        self.state.layers.push(surface);
    }

    /// Opens a bar like [`Self::open_bar`], not exclusive, that takes the
    /// keyboard when clicked, as a panel with a search field does.
    ///
    /// # Panics
    ///
    /// Panics if vaht has no layer shell or the connection fails.
    pub fn open_keyboard_bar(&mut self, layer: TestLayer, thickness: u32, color: [u8; 3]) {
        let surface = self.open_layer(layer, color, |bar| {
            bar.set_anchor(Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
            bar.set_size(0, thickness);
            bar.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
        });
        self.state.layers.push(surface);
    }

    /// Commits every layer surface again, unchanged, as a panel redrawing
    /// its clock does.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn commit_layers(&mut self) {
        for layer in &self.state.layers {
            layer.commit();
        }
        self.roundtrip();
    }

    /// Returns what has the client's keyboard focus, once vaht has handled
    /// everything sent so far.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn keyboard_focus(&mut self) -> KeyboardFocus {
        self.roundtrip();
        let Some(focused) = &self.state.keyboard_focus else {
            return KeyboardFocus::Nothing;
        };
        if let Some(position) = self
            .state
            .windows
            .iter()
            .position(|window| window.wl_surface() == focused)
        {
            return KeyboardFocus::Window(position);
        }
        self.state
            .layers
            .iter()
            .position(|layer| layer.wl_surface() == focused)
            .map_or(KeyboardFocus::Nothing, KeyboardFocus::Layer)
    }

    /// Opens a layer surface covering the whole output in `layer`, filled
    /// with `color`, as a wallpaper is. Waits until vaht has its first
    /// frame.
    ///
    /// # Panics
    ///
    /// Panics if vaht has no layer shell or the connection fails.
    pub fn open_backdrop(&mut self, layer: TestLayer, color: [u8; 3]) {
        let surface = self.open_layer(layer, color, |backdrop| {
            backdrop.set_anchor(Anchor::all());
            backdrop.set_exclusive_zone(-1);
        });
        self.state.layers.push(surface);
    }

    /// Creates a layer surface in `layer`, lets `arrange` set it up, and
    /// waits until it has drawn and vaht has the frame.
    fn open_layer(
        &mut self,
        layer: TestLayer,
        color: [u8; 3],
        arrange: impl FnOnce(&LayerSurface),
    ) -> LayerSurface {
        let handle = self.queue.handle();
        let surface = self.state.compositor.create_surface(&handle);
        let layer_surface = self.state.layer_shell.create_layer_surface(
            &handle,
            surface,
            layer.into(),
            Some("vaht.test"),
            None,
        );
        arrange(&layer_surface);
        layer_surface.commit();
        self.state
            .layer_colors
            .push((layer_surface.wl_surface().clone(), color));
        let expected = self.state.first_draws + 1;
        self.dispatch_until(|state| state.first_draws >= expected);
        self.roundtrip();
        layer_surface
    }

    /// Returns the interface names of the globals vaht advertises, sorted.
    #[must_use]
    pub fn global_interfaces(&self) -> Vec<String> {
        let mut interfaces: Vec<String> = self
            .state
            .registry
            .globals()
            .map(|global| global.interface.clone())
            .collect();
        interfaces.sort();
        interfaces.dedup();
        interfaces
    }

    /// Presses `button` (a Linux input code, such as [`BUTTON_LEFT`]).
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn press(&mut self, button: u32) {
        self.send_button(button, wl_pointer::ButtonState::Pressed);
    }

    /// Releases `button`.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn release(&mut self, button: u32) {
        self.send_button(button, wl_pointer::ButtonState::Released);
    }

    /// Presses and releases `button` at `x`, `y`.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn click(&mut self, x: u32, y: u32, button: u32) {
        self.move_pointer_to(x, y);
        self.press(button);
        self.release(button);
    }

    /// Presses and holds `button` at `from`, moves to `to`, and releases.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn drag(&mut self, from: (u32, u32), to: (u32, u32), button: u32) {
        self.move_pointer_to(from.0, from.1);
        self.press(button);
        self.move_pointer_to(to.0, to.1);
        self.release(button);
    }

    /// Holds Alt down until [`Self::release_modifiers`].
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn hold_alt(&mut self) {
        self.input.hold_alt();
        self.roundtrip();
    }

    /// Presses `key` (a Linux input code from [`keys`]) and holds it until
    /// [`Self::release_key`].
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn press_key(&mut self, key: u32) {
        self.input.key(key, KeyMotion::Press);
        self.roundtrip();
    }

    /// Releases `key`.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn release_key(&mut self, key: u32) {
        self.input.key(key, KeyMotion::Release);
        self.roundtrip();
    }

    /// Presses `chord`'s keys in order, then releases them in reverse, as a
    /// person types `Super+T`.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn type_chord(&mut self, chord: &[u32]) {
        for &key in chord {
            self.press_key(key);
        }
        for &key in chord.iter().rev() {
            self.release_key(key);
        }
    }

    /// Types `text` key by key, each pressed and released.
    ///
    /// # Panics
    ///
    /// Panics if `text` has a character [`keys::typing`] has no key for,
    /// or the connection to vaht fails.
    pub fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            let (key, is_shifted) =
                keys::typing(character).unwrap_or_else(|| panic!("no key types {character:?}"));
            if is_shifted {
                self.type_chord(&[keys::LEFT_SHIFT, key]);
            } else {
                self.type_chord(&[key]);
            }
        }
    }

    /// Lets go of every held modifier.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn release_modifiers(&mut self) {
        self.input.release_modifiers();
        self.roundtrip();
    }

    /// Captures the (first) output as vaht shows it now.
    ///
    /// # Panics
    ///
    /// Panics if the capture fails or the connection to vaht does.
    pub fn screenshot(&mut self) -> Screenshot {
        let output = self.output();
        self.capture(&output, Options::empty())
    }

    /// Captures the (first) output as vaht shows it now, with the pointer
    /// drawn, as a screen recorder asks.
    ///
    /// # Panics
    ///
    /// Panics if the capture fails or the connection to vaht does.
    pub fn screenshot_with_pointer(&mut self) -> Screenshot {
        let output = self.output();
        self.capture(&output, Options::PaintCursors)
    }

    /// Captures the output named `name` (such as `HEADLESS-2`) as vaht
    /// shows it now.
    ///
    /// # Panics
    ///
    /// Panics if vaht has no such output, or the capture fails or the
    /// connection to vaht does.
    pub fn screenshot_of(&mut self, name: &str) -> Screenshot {
        // An output just plugged in is announced, then described.
        self.roundtrip();
        self.roundtrip();
        let output = self
            .state
            .outputs
            .outputs()
            .find(|output| {
                self.state
                    .outputs
                    .info(output)
                    .is_some_and(|info| info.name.as_deref() == Some(name))
            })
            .unwrap_or_else(|| panic!("vaht has an output named {name}"));
        self.capture(&output, Options::empty())
    }

    /// Captures `output` as vaht shows it now, as `options` ask.
    fn capture(&mut self, output: &wl_output::WlOutput, options: Options) -> Screenshot {
        let handle = self.queue.handle();
        let source: ExtImageCaptureSourceV1 = self
            .state
            .capture_sources
            .create_source(output, &handle, NoEvents);
        let session =
            self.state
                .capture
                .create_session(&source, options, &handle, CaptureSessionEvents);
        source.destroy();
        self.state.capture_size = None;
        self.dispatch_until(|state| state.capture_size.is_some());
        let (width, height) = self
            .state
            .capture_size
            .expect("the session announced a size");
        let signed = |value: u32| i32::try_from(value).expect("output sizes fit in i32");
        let (buffer, _) = self
            .state
            .pool
            .create_buffer(
                signed(width),
                signed(height),
                signed(width * BYTES_PER_PIXEL),
                wl_shm::Format::Argb8888,
            )
            .expect("the pool grows to fit a screenshot");
        let frame = session.create_frame(&handle, CaptureFrameEvents);
        frame.attach_buffer(buffer.wl_buffer());
        frame.damage_buffer(0, 0, signed(width), signed(height));
        frame.capture();
        self.state.capture_result = None;
        self.dispatch_until(|state| state.capture_result.is_some());
        let result = self
            .state
            .capture_result
            .take()
            .expect("the frame finished");
        assert!(result.is_ok(), "vaht failed the capture: {result:?}");
        let pixels = buffer
            .canvas(&mut self.state.pool)
            .expect("the buffer is still the pool's")
            .to_vec();
        frame.destroy();
        session.destroy();
        Screenshot::new(width, height, pixels)
    }

    /// Sends a button event and waits until vaht has processed it.
    fn send_button(&mut self, button: u32, state: wl_pointer::ButtonState) {
        self.input.button(button, state);
        self.roundtrip();
    }

    /// Returns vaht's first output.
    fn output(&self) -> wl_output::WlOutput {
        self.state
            .outputs
            .outputs()
            .next()
            .expect("vaht has an output")
    }

    /// Returns the size of the area every output fits in, from the
    /// global origin, in pixels: what absolute pointer motion spans.
    fn layout_size(&self) -> (u32, u32) {
        let unsigned = |value: i32| {
            u32::try_from(value).expect("the layout lies right of and below the origin")
        };
        self.state
            .outputs
            .outputs()
            .map(|output| {
                let info = self
                    .state
                    .outputs
                    .info(&output)
                    .expect("the output's details arrived");
                let mode = info
                    .modes
                    .iter()
                    .find(|mode| mode.current)
                    .expect("the output has a current mode");
                (
                    unsigned(info.location.0 + mode.dimensions.0),
                    unsigned(info.location.1 + mode.dimensions.1),
                )
            })
            .fold((0, 0), |(width, height), (right, bottom)| {
                (width.max(right), height.max(bottom))
            })
    }

    /// Waits until vaht has processed everything sent so far.
    fn roundtrip(&mut self) {
        self.queue
            .roundtrip(&mut self.state)
            .expect("the connection to vaht stays up");
    }

    /// Reads vaht's events until `done` says so, or `patience` runs out;
    /// returns whether `done` said so. It waits on the connection, never
    /// spinning.
    fn dispatch_for(&mut self, patience: Duration, done: impl Fn(&ClientState) -> bool) -> bool {
        let deadline = std::time::Instant::now() + patience;
        loop {
            self.queue
                .dispatch_pending(&mut self.state)
                .expect("the connection to vaht stays up");
            if done(&self.state) {
                return true;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            self.queue.flush().expect("the connection to vaht stays up");
            if let Some(guard) = self.queue.prepare_read() {
                let mut fds = [nix::poll::PollFd::new(
                    guard.connection_fd(),
                    nix::poll::PollFlags::POLLIN,
                )];
                let millis = u16::try_from(left.as_millis()).unwrap_or(u16::MAX);
                let ready =
                    nix::poll::poll(&mut fds, nix::poll::PollTimeout::from(millis)).unwrap_or(0);
                if ready > 0 {
                    let _ = guard.read();
                }
            }
        }
    }

    /// Handles events until `done` holds.
    fn dispatch_until(&mut self, done: impl Fn(&ClientState) -> bool) {
        while !done(&self.state) {
            self.queue
                .blocking_dispatch(&mut self.state)
                .expect("the connection to vaht stays up");
        }
    }
}

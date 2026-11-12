//! A Wayland client that opens windows, drives a virtual pointer and takes
//! screenshots, all on one connection.
//!
//! Requests on one connection reach vaht in order, so a screenshot taken
//! after pointer input shows that input's effect.

use std::fs::File;
use std::os::fd::AsFd as _;
use std::os::unix::net::UnixStream;
use std::path::Path;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, FrameCallbackData};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::globals::{GlobalList, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use smithay_client_toolkit::reexports::protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use smithay_client_toolkit::reexports::client::{Connection, EventQueue, QueueHandle};
use smithay_client_toolkit::reexports::protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use smithay_client_toolkit::reexports::protocols::ext::image_capture_source::v1::client::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1;
use smithay_client_toolkit::reexports::protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1};
use smithay_client_toolkit::reexports::protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options};
use smithay_client_toolkit::reexports::protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1};
use smithay_client_toolkit::reexports::protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use smithay_client_toolkit::reexports::protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::WaylandSurface as _;
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::xdg::window::{Window, WindowConfigure, WindowDecorations, WindowHandler};
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, registry_handlers};

use crate::screenshot::Screenshot;

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

/// `wl_keyboard.keymap_format.no_keymap`: the keyboard uses the seat's keymap.
const NO_KEYMAP_FORMAT: u32 = 0;

/// The modifier mask of Alt in the default xkb keymap, where Alt is Mod1:
/// the fourth real modifier, after Shift, Lock and Control.
const ALT_MODIFIER_MASK: u32 = 1 << 3;

/// Bytes per pixel of an ARGB8888 buffer.
const BYTES_PER_PIXEL: u32 = 4;

/// A client connected to vaht.
#[derive(Debug)]
pub struct TestClient {
    /// The client's event queue.
    queue: EventQueue<ClientState>,
    /// What the client knows and has.
    state: ClientState,
    /// The pointer it moves, and the output it moves on.
    pointer: ZwlrVirtualPointerV1,
    /// The keyboard it holds modifiers with.
    keyboard: ZwpVirtualKeyboardV1,
    /// The empty file sent as the keyboard's keymap; kept open until sent.
    _no_keymap: File,
    /// The milliseconds clock stamped on pointer events.
    time: u32,
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
        let shm = Shm::bind(&globals, &handle).expect("vaht offers wl_shm");
        let pool_size = usize::try_from(WINDOW_SIZE.0 * WINDOW_SIZE.1 * BYTES_PER_PIXEL)
            .expect("a small buffer size fits in usize");
        let pointer_manager: ZwlrVirtualPointerManagerV1 =
            bind(&globals, &handle, "zwlr_virtual_pointer_manager_v1");
        let pointer = pointer_manager.create_virtual_pointer(None, &handle, NoEvents);
        let seat: wl_seat::WlSeat = bind(&globals, &handle, "wl_seat");
        let keyboard_manager: ZwpVirtualKeyboardManagerV1 =
            bind(&globals, &handle, "zwp_virtual_keyboard_manager_v1");
        let keyboard = keyboard_manager.create_virtual_keyboard(&seat, &handle, NoEvents);
        // `no_keymap`: the keyboard's modifier masks are read with vaht's own
        // keymap, so no keymap needs writing. The protocol still wants a file.
        let no_keymap = File::open("/dev/null").expect("/dev/null opens");
        keyboard.keymap(NO_KEYMAP_FORMAT, no_keymap.as_fd(), 0);
        let mut state = ClientState {
            registry: RegistryState::new(&globals),
            outputs: OutputState::new(&globals, &handle),
            compositor: CompositorState::bind(&globals, &handle)
                .expect("vaht offers wl_compositor"),
            xdg_shell: XdgShell::bind(&globals, &handle).expect("vaht offers xdg_wm_base"),
            capture_sources: bind(
                &globals,
                &handle,
                "ext_output_image_capture_source_manager_v1",
            ),
            capture: bind(&globals, &handle, "ext_image_copy_capture_manager_v1"),
            pool: SlotPool::new(pool_size, &shm).expect("a shared-memory pool"),
            shm,
            windows: Vec::new(),
            frames_shown: 0,
            close_requests: 0,
            capture_size: None,
            capture_result: None,
        };
        // Learn the outputs before anything needs them.
        queue
            .roundtrip(&mut state)
            .expect("the connection to vaht stays up");
        Self {
            queue,
            state,
            pointer,
            keyboard,
            _no_keymap: no_keymap,
            time: 0,
        }
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
            window.set_title(format!("test window {number}"));
            window.set_app_id("vaht.test");
            window.commit();
            self.state.windows.push(window);
        }
        let expected = self.state.frames_shown + count;
        self.dispatch_until(|state| state.frames_shown >= expected);
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

    /// Moves the pointer to `x`, `y` on the output.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn move_pointer_to(&mut self, x: u32, y: u32) {
        let (width, height) = self.output_size();
        let time = self.tick();
        self.pointer.motion_absolute(time, x, y, width, height);
        self.pointer.frame();
        self.roundtrip();
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
        self.keyboard.modifiers(ALT_MODIFIER_MASK, 0, 0, 0);
        self.roundtrip();
    }

    /// Lets go of every held modifier.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn release_modifiers(&mut self) {
        self.keyboard.modifiers(0, 0, 0, 0);
        self.roundtrip();
    }

    /// Captures the output as vaht shows it now.
    ///
    /// # Panics
    ///
    /// Panics if the capture fails or the connection to vaht does.
    pub fn screenshot(&mut self) -> Screenshot {
        let handle = self.queue.handle();
        let output = self.output();
        let source: ExtImageCaptureSourceV1 = self
            .state
            .capture_sources
            .create_source(&output, &handle, NoEvents);
        let session = self.state.capture.create_session(
            &source,
            Options::empty(),
            &handle,
            CaptureSessionEvents,
        );
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
        let time = self.tick();
        self.pointer.button(time, button, state);
        self.pointer.frame();
        self.roundtrip();
    }

    /// Returns the next pointer event time, a millisecond after the last.
    const fn tick(&mut self) -> u32 {
        self.time += 1;
        self.time
    }

    /// Returns vaht's (only) output.
    fn output(&self) -> wl_output::WlOutput {
        self.state
            .outputs
            .outputs()
            .next()
            .expect("vaht has an output")
    }

    /// Returns the output's size in pixels.
    fn output_size(&self) -> (u32, u32) {
        let info = self
            .state
            .outputs
            .info(&self.output())
            .expect("the output's details arrived");
        let mode = info
            .modes
            .iter()
            .find(|mode| mode.current)
            .expect("the output has a current mode");
        let unsigned = |value: i32| u32::try_from(value).expect("mode sizes are positive");
        (unsigned(mode.dimensions.0), unsigned(mode.dimensions.1))
    }

    /// Waits until vaht has processed everything sent so far.
    fn roundtrip(&mut self) {
        self.queue
            .roundtrip(&mut self.state)
            .expect("the connection to vaht stays up");
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

/// Binds the global `name`, which vaht must offer.
fn bind<I>(globals: &GlobalList, handle: &QueueHandle<ClientState>, name: &str) -> I
where
    I: smithay_client_toolkit::reexports::client::Proxy + 'static,
    NoEvents: Dispatch2<I, ClientState>,
{
    globals
        .bind(handle, 1..=1, NoEvents)
        .unwrap_or_else(|error| panic!("vaht offers {name}: {error}"))
}

/// The client's state, which the toolkit's handlers update.
#[derive(Debug)]
struct ClientState {
    /// The globals vaht offers.
    registry: RegistryState,
    /// vaht's outputs.
    outputs: OutputState,
    /// Creates surfaces.
    compositor: CompositorState,
    /// Creates windows.
    xdg_shell: XdgShell,
    /// Turns outputs into capture sources.
    capture_sources: ExtOutputImageCaptureSourceManagerV1,
    /// Captures them.
    capture: ExtImageCopyCaptureManagerV1,
    /// Shared-memory buffers.
    shm: Shm,
    /// Where buffers are allocated.
    pool: SlotPool,
    /// The open windows.
    windows: Vec<Window>,
    /// How many frame callbacks arrived: each one is a window shown.
    frames_shown: usize,
    /// How many times vaht asked to close a window.
    close_requests: usize,
    /// The buffer size the current capture session asked for.
    capture_size: Option<(u32, u32)>,
    /// How the current capture ended.
    capture_result: Option<Result<(), String>>,
}

impl ClientState {
    /// Fills `window`, `size` pixels large, with [`WINDOW_COLOR`] and asks
    /// to hear when it is shown.
    fn draw(&mut self, window: &Window, handle: &QueueHandle<Self>, size: (u32, u32)) {
        let width = i32::try_from(size.0).expect("window sizes fit in i32");
        let height = i32::try_from(size.1).expect("window sizes fit in i32");
        let stride = width * i32::try_from(BYTES_PER_PIXEL).expect("4 fits in i32");
        let (buffer, canvas) = self
            .pool
            .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
            .expect("the pool has room");
        let [red, green, blue] = WINDOW_COLOR;
        let (pixels, _) = canvas.as_chunks_mut::<4>();
        for pixel in pixels {
            *pixel = [blue, green, red, u8::MAX];
        }
        let surface = window.wl_surface();
        surface.frame(handle, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface).expect("a fresh buffer attaches");
        surface.damage_buffer(0, 0, width, height);
        window.commit();
    }
}

/// User data for objects whose events the client ignores or has none.
#[derive(Debug, Clone, Copy)]
struct NoEvents;

/// Implements [`Dispatch2`] for interfaces whose events are ignored.
macro_rules! ignore_events {
    ($($interface:ty),+ $(,)?) => {
        $(
            impl Dispatch2<$interface, ClientState> for NoEvents {
                fn event(
                    &self,
                    _: &mut ClientState,
                    _: &$interface,
                    _: <$interface as smithay_client_toolkit::reexports::client::Proxy>::Event,
                    _: &Connection,
                    _: &QueueHandle<ClientState>,
                ) {
                }
            }
        )+
    };
}

ignore_events!(
    wl_seat::WlSeat,
    ZwpVirtualKeyboardManagerV1,
    ZwpVirtualKeyboardV1,
    ZwlrVirtualPointerManagerV1,
    ZwlrVirtualPointerV1,
    ExtOutputImageCaptureSourceManagerV1,
    ExtImageCaptureSourceV1,
    ExtImageCopyCaptureManagerV1,
);

/// User data of a capture session: records the buffer size it asks for.
#[derive(Debug, Clone, Copy)]
struct CaptureSessionEvents;

impl Dispatch2<ExtImageCopyCaptureSessionV1, ClientState> for CaptureSessionEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        if let ext_image_copy_capture_session_v1::Event::BufferSize { width, height } = event {
            state.capture_size = Some((width, height));
        }
    }
}

/// User data of a capture frame: records how it ended.
#[derive(Debug, Clone, Copy)]
struct CaptureFrameEvents;

impl Dispatch2<ExtImageCopyCaptureFrameV1, ClientState> for CaptureFrameEvents {
    fn event(
        &self,
        state: &mut ClientState,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &Connection,
        _: &QueueHandle<ClientState>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "only the frame's outcome matters to a test"
        )]
        match event {
            ext_image_copy_capture_frame_v1::Event::Ready => state.capture_result = Some(Ok(())),
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => {
                state.capture_result = Some(Err(format!("{reason:?}")));
            }
            _ => {}
        }
    }
}

impl CompositorHandler for ClientState {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frames_shown += 1;
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for ClientState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for ClientState {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.close_requests += 1;
    }

    fn configure(
        &mut self,
        _: &Connection,
        handle: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        // Take the size vaht asks for, as real clients do; with none, pick.
        let (width, height) = configure.new_size;
        let size = (
            width.map_or(WINDOW_SIZE.0, std::num::NonZeroU32::get),
            height.map_or(WINDOW_SIZE.1, std::num::NonZeroU32::get),
        );
        self.draw(window, handle, size);
    }
}

impl ShmHandler for ClientState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for ClientState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState];
}

delegate_registry!(ClientState);
smithay_client_toolkit::delegate_dispatch2!(ClientState);

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

use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm};
use smithay_client_toolkit::reexports::client::{Connection, EventQueue};
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
    CaptureFrameEvents, CaptureSessionEvents, ClientState, KeyboardEvents, NoEvents, bind,
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

    /// Handles events until `done` holds.
    fn dispatch_until(&mut self, done: impl Fn(&ClientState) -> bool) {
        while !done(&self.state) {
            self.queue
                .blocking_dispatch(&mut self.state)
                .expect("the connection to vaht stays up");
        }
    }
}

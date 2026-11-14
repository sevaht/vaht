//! An X11 program, as tests drive one through Xwayland.
//!
//! [`X11Client`] opens windows filled with one color, with a title, a class
//! and its process id, as an ordinary X program's are, and override-
//! redirect windows (an X program's menus) where it says; it hears vaht
//! ask a window to close, and asks the X server where it thinks a window
//! is, which is where an X program puts its menus.

use std::os::fd::AsFd as _;
use std::time::Duration;

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// How long a test waits for the X server or vaht to answer.
const PATIENCE: Duration = Duration::from_secs(10);

/// A connection to Xwayland, with the windows it opened.
#[derive(Debug)]
pub struct X11Client {
    /// The connection.
    connection: RustConnection,
    /// The root window of its screen.
    root: Window,
    /// `WM_PROTOCOLS`.
    wm_protocols: u32,
    /// `WM_DELETE_WINDOW`.
    wm_delete_window: u32,
    /// The windows vaht asked to close, so far.
    close_requests: Vec<Window>,
    /// The windows that received a key press, so far.
    key_presses: Vec<Window>,
}

impl X11Client {
    /// Connects to the X display `display`, such as `:1`.
    ///
    /// # Panics
    ///
    /// Panics if the display cannot be reached.
    #[must_use]
    pub fn connect(display: &str) -> Self {
        let (connection, screen) =
            x11rb::connect(Some(display)).expect("Xwayland accepts the connection");
        let root = connection.setup().roots[screen].root;
        let atom = |name: &[u8]| {
            connection
                .intern_atom(false, name)
                .expect("the request is sent")
                .reply()
                .expect("the atom exists")
                .atom
        };
        let wm_protocols = atom(b"WM_PROTOCOLS");
        let wm_delete_window = atom(b"WM_DELETE_WINDOW");
        Self {
            connection,
            root,
            wm_protocols,
            wm_delete_window,
            close_requests: Vec::new(),
            key_presses: Vec::new(),
        }
    }

    /// Opens and maps a window `size` pixels, all `color`, titled `title`,
    /// of class `class`, which takes `WM_DELETE_WINDOW`.
    ///
    /// # Panics
    ///
    /// Panics if the X server refuses a request.
    pub fn open_window(
        &mut self,
        size: (u16, u16),
        color: [u8; 3],
        title: &str,
        class: &str,
    ) -> Window {
        let window = self.create(((0, 0), size), color, false);
        let connection = &self.connection;
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                title.as_bytes(),
            )
            .expect("the request is sent");
        let mut wm_class = format!("{class}\0{class}\0").into_bytes();
        wm_class.shrink_to_fit();
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                &wm_class,
            )
            .expect("the request is sent");
        connection
            .change_property32(
                PropMode::REPLACE,
                window,
                self.wm_protocols,
                AtomEnum::ATOM,
                &[self.wm_delete_window],
            )
            .expect("the request is sent");
        let pid = self.atom(b"_NET_WM_PID");
        connection
            .change_property32(
                PropMode::REPLACE,
                window,
                pid,
                AtomEnum::CARDINAL,
                &[std::process::id()],
            )
            .expect("the request is sent");
        connection.map_window(window).expect("the request is sent");
        connection.flush().expect("the requests are sent");
        window
    }

    /// Opens and maps a window like [`Self::open_window`] whose Motif hints
    /// ask for no decorations: it draws its own, as a frameless Electron
    /// window does.
    ///
    /// # Panics
    ///
    /// Panics if the X server refuses a request.
    pub fn open_self_decorated_window(
        &mut self,
        size: (u16, u16),
        color: [u8; 3],
        title: &str,
    ) -> Window {
        let window = self.create(((0, 0), size), color, false);
        let hints = self.atom(b"_MOTIF_WM_HINTS");
        // flags: decorations given; decorations: none.
        self.connection
            .change_property32(PropMode::REPLACE, window, hints, hints, &[2, 0, 0, 0, 0])
            .expect("the request is sent");
        self.connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                title.as_bytes(),
            )
            .expect("the request is sent");
        self.connection
            .map_window(window)
            .expect("the request is sent");
        self.connection.flush().expect("the requests are sent");
        window
    }

    /// Shapes `window` to `rectangles` (`(x, y, width, height)` relative
    /// to it), as Wine's layered windows do.
    ///
    /// # Panics
    ///
    /// Panics if the X server refuses the request.
    pub fn shape(&mut self, window: Window, rectangles: &[(i16, i16, u16, u16)]) {
        let rectangles: Vec<_> = rectangles
            .iter()
            .map(
                |&(x, y, width, height)| x11rb::protocol::xproto::Rectangle {
                    x,
                    y,
                    width,
                    height,
                },
            )
            .collect();
        self.connection
            .shape_rectangles(
                shape::SO::SET,
                shape::SK::BOUNDING,
                x11rb::protocol::xproto::ClipOrdering::UNSORTED,
                window,
                0,
                0,
                &rectangles,
            )
            .expect("the request is sent");
        self.connection.flush().expect("the request is sent");
    }

    /// Opens and maps an override-redirect window (a menu) covering `area`
    /// (`(x, y)` and `(width, height)` on the X screen), all `color`.
    ///
    /// # Panics
    ///
    /// Panics if the X server refuses a request.
    pub fn open_menu(&mut self, area: ((i16, i16), (u16, u16)), color: [u8; 3]) -> Window {
        let window = self.create(area, color, true);
        self.connection
            .map_window(window)
            .expect("the request is sent");
        self.connection.flush().expect("the requests are sent");
        window
    }

    /// Asks the window manager to move `window` with the pointer, which is
    /// at `pointer` on the X screen with its left button held: what a
    /// frameless window's own title bar does (`_NET_WM_MOVERESIZE`).
    ///
    /// # Panics
    ///
    /// Panics if the X server refuses the request.
    pub fn request_move(&mut self, window: Window, pointer: (i16, i16)) {
        /// `_NET_WM_MOVERESIZE_MOVE`.
        const MOVE: u32 = 8;
        let move_resize = self.atom(b"_NET_WM_MOVERESIZE");
        let to_card = |value: i16| u32::from_ne_bytes(i32::from(value).to_ne_bytes());
        let event = x11rb::protocol::xproto::ClientMessageEvent::new(
            32,
            window,
            move_resize,
            [to_card(pointer.0), to_card(pointer.1), MOVE, 1, 1],
        );
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .expect("the request is sent");
        self.connection.flush().expect("the request is sent");
    }

    /// Returns where the X server thinks the pointer is on its screen:
    /// what an X program asks to know what it hovers.
    ///
    /// # Panics
    ///
    /// Panics if the X server does not answer.
    #[must_use]
    pub fn pointer_position(&self) -> (i16, i16) {
        let reply = self
            .connection
            .query_pointer(self.root)
            .expect("the request is sent")
            .reply()
            .expect("the X server answers");
        (reply.root_x, reply.root_y)
    }

    /// Returns the X resources the server holds (`RESOURCE_MANAGER` on the
    /// root window), as X programs read their settings.
    ///
    /// # Panics
    ///
    /// Panics if the X server does not answer.
    #[must_use]
    pub fn resources(&self) -> String {
        let reply = self
            .connection
            .get_property(
                false,
                self.root,
                AtomEnum::RESOURCE_MANAGER,
                AtomEnum::STRING,
                0,
                u32::MAX / 4,
            )
            .expect("the request is sent")
            .reply()
            .expect("the X server answers");
        String::from_utf8_lossy(&reply.value).into_owned()
    }

    /// Returns where the X server thinks `window`'s top left corner is on
    /// its screen: what an X program places its menus by.
    ///
    /// # Panics
    ///
    /// Panics if the X server does not answer.
    #[must_use]
    pub fn position_of(&self, window: Window) -> (i16, i16) {
        let reply = self
            .connection
            .translate_coordinates(window, self.root, 0, 0)
            .expect("the request is sent")
            .reply()
            .expect("the X server answers");
        (reply.dst_x, reply.dst_y)
    }

    /// Waits until the X server has `window` at `position`, as vaht told
    /// it; returns where it is in the end.
    ///
    /// # Panics
    ///
    /// Panics if the X server does not answer.
    pub fn wait_for_position(&mut self, window: Window, position: (i16, i16)) -> (i16, i16) {
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            let now = self.position_of(window);
            if now == position || std::time::Instant::now() >= deadline {
                return now;
            }
            // A move arrives as an event (ConfigureNotify); wait for one.
            self.wait_for_events(deadline.saturating_duration_since(std::time::Instant::now()));
        }
    }

    /// Waits until vaht asks `window` to close (`WM_DELETE_WINDOW`);
    /// returns whether it did in time.
    pub fn wait_for_close_request(&mut self, window: Window) -> bool {
        let deadline = std::time::Instant::now() + PATIENCE;
        while !self.close_requests.contains(&window) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            self.wait_for_events(left);
        }
        true
    }

    /// Waits until `window` receives a key press; returns whether it did in
    /// time.
    pub fn wait_for_key_press(&mut self, window: Window) -> bool {
        let deadline = std::time::Instant::now() + PATIENCE;
        while !self.key_presses.contains(&window) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            self.wait_for_events(left);
        }
        true
    }

    /// Creates a window covering `area`, all `color`, override-redirect or
    /// not; it hears structure changes.
    fn create(
        &self,
        area: ((i16, i16), (u16, u16)),
        color: [u8; 3],
        is_override_redirect: bool,
    ) -> Window {
        let ((x, y), (width, height)) = area;
        let window = self.connection.generate_id().expect("an id is free");
        let [red, green, blue] = color;
        let pixel = u32::from_be_bytes([0, red, green, blue]);
        let aux = CreateWindowAux::new()
            .background_pixel(pixel)
            .override_redirect(u32::from(is_override_redirect))
            .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::KEY_PRESS);
        self.connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                self.root,
                x,
                y,
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &aux,
            )
            .expect("the request is sent");
        window
    }

    /// Returns the atom `name`.
    fn atom(&self, name: &[u8]) -> u32 {
        self.connection
            .intern_atom(false, name)
            .expect("the request is sent")
            .reply()
            .expect("the X server answers")
            .atom
    }

    /// Waits up to `timeout` for the X server to send something, then
    /// reads every event, noting close requests.
    fn wait_for_events(&mut self, timeout: Duration) {
        let mut fds = [PollFd::new(
            self.connection.stream().as_fd(),
            PollFlags::POLLIN,
        )];
        let millis = u16::try_from(timeout.as_millis()).unwrap_or(u16::MAX);
        let _ = poll(&mut fds, PollTimeout::from(millis));
        while let Some(event) = self
            .connection
            .poll_for_event()
            .expect("the connection holds")
        {
            #[expect(
                clippy::wildcard_enum_match_arm,
                reason = "of the X server's hundred events, a test hears two"
            )]
            match event {
                Event::ClientMessage(message)
                    if message.type_ == self.wm_protocols
                        && message.data.as_data32()[0] == self.wm_delete_window =>
                {
                    self.close_requests.push(message.window);
                }
                Event::KeyPress(press) => self.key_presses.push(press.event),
                _ => {}
            }
        }
    }
}

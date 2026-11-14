//! A night-light tool (`zwlr_gamma_control_v1`), as wlsunset is, as tests
//! drive it.

use std::io::{Seek as _, Write as _};
use std::os::fd::AsFd as _;
use std::path::Path;

use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_registry};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, EventQueue, Proxy as _, QueueHandle};
use smithay_client_toolkit::reexports::protocols_wlr::gamma_control::v1::client::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1;
use smithay_client_toolkit::reexports::protocols_wlr::gamma_control::v1::client::zwlr_gamma_control_v1::{self, ZwlrGammaControlV1};

/// A connection controlling the colors of vaht's first output.
#[derive(Debug)]
pub struct GammaControl {
    /// The events vaht sends.
    queue: EventQueue<GammaState>,
    /// What they said.
    state: GammaState,
    /// The control.
    control: ZwlrGammaControlV1,
}

/// What vaht said of the control.
#[derive(Debug, Default)]
struct GammaState {
    /// The ramps' size, once vaht has said.
    size: Option<u32>,
    /// Whether the control failed.
    has_failed: bool,
}

impl GammaControl {
    /// Connects to vaht at `socket` and asks to control its first output's
    /// colors.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be reached or offers no gamma control.
    #[must_use]
    pub fn connect(socket: &Path) -> Self {
        let stream =
            std::os::unix::net::UnixStream::connect(socket).expect("vaht's socket accepts");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) =
            registry_queue_init::<GammaState>(&connection).expect("the registry is read");
        let handle = queue.handle();
        let output: wl_output::WlOutput = globals
            .bind(&handle, 1..=4, ())
            .expect("vaht offers an output");
        let manager: ZwlrGammaControlManagerV1 = globals
            .bind(&handle, 1..=1, ())
            .expect("vaht offers zwlr_gamma_control_manager_v1");
        let control = manager.get_gamma_control(&output, &handle, ());
        let mut state = GammaState::default();
        queue.roundtrip(&mut state).expect("vaht answers");
        Self {
            queue,
            state,
            control,
        }
    }

    /// Returns the ramps' size vaht told, if it did.
    #[must_use]
    pub const fn size(&self) -> Option<u32> {
        self.state.size
    }

    /// Returns whether the control failed, after reading what vaht sent.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn has_failed(&mut self) -> bool {
        self.queue.roundtrip(&mut self.state).expect("vaht answers");
        self.state.has_failed
    }

    /// Sends `values`, red's, green's then blue's.
    ///
    /// # Panics
    ///
    /// Panics if the ramps cannot be written or the connection fails.
    pub fn send_ramps(&mut self, values: &[u16]) {
        let mut file = tempfile::tempfile().expect("a file for the ramps");
        let bytes: Vec<u8> = values
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect();
        file.write_all(&bytes).expect("written");
        file.rewind().expect("rewound");
        self.control.set_gamma(file.as_fd());
        self.queue.roundtrip(&mut self.state).expect("vaht answers");
    }

    /// Lets the control go, as a night-light tool does when it quits.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn release(mut self) {
        self.control.destroy();
        self.queue.roundtrip(&mut self.state).expect("vaht answers");
        assert!(!self.control.is_alive());
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for GammaState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, ()> for GammaState {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for GammaState {
    fn event(
        _: &mut Self,
        _: &ZwlrGammaControlManagerV1,
        _: <ZwlrGammaControlManagerV1 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrGammaControlV1, ()> for GammaState {
    fn event(
        state: &mut Self,
        _: &ZwlrGammaControlV1,
        event: zwlr_gamma_control_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_gamma_control_v1::Event::GammaSize { size } => state.size = Some(size),
            zwlr_gamma_control_v1::Event::Failed => state.has_failed = true,
            _ => {}
        }
    }
}

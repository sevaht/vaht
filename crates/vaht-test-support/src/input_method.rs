//! An input method (`zwp_input_method_v2`), as fcitx5 or ibus is, as tests
//! drive it.
//!
//! [`InputMethod`] connects on its own and types text into whatever text
//! field vaht says is active.

use std::path::Path;
use std::time::Instant;

use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::{wl_registry, wl_seat};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, EventQueue, QueueHandle};
use smithay_client_toolkit::reexports::protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use smithay_client_toolkit::reexports::protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::{self, ZwpInputMethodV2};

use crate::wait::PATIENCE;

/// A connection acting as the seat's input method.
#[derive(Debug)]
pub struct InputMethod {
    /// The events vaht sends.
    queue: EventQueue<MethodState>,
    /// What they said.
    state: MethodState,
    /// The input method.
    method: ZwpInputMethodV2,
}

/// What vaht said to the input method.
#[derive(Debug, Default)]
struct MethodState {
    /// Whether a text field is active, as of the last `done`.
    is_active: bool,
    /// Whether one is active since the last `done`.
    pending_active: bool,
    /// How many `done`s vaht sent: the serial a commit names.
    done_count: u32,
    /// Whether vaht said another input method is in use.
    is_unavailable: bool,
}

impl InputMethod {
    /// Connects to vaht at `socket` as the seat's input method.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be reached or offers no input methods.
    #[must_use]
    pub fn connect(socket: &Path) -> Self {
        let stream =
            std::os::unix::net::UnixStream::connect(socket).expect("vaht's socket accepts");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) =
            registry_queue_init::<MethodState>(&connection).expect("the registry is read");
        let handle = queue.handle();
        let seat: wl_seat::WlSeat = globals
            .bind(&handle, 1..=9, ())
            .expect("vaht offers wl_seat");
        let manager: ZwpInputMethodManagerV2 = globals
            .bind(&handle, 1..=1, ())
            .expect("vaht offers zwp_input_method_manager_v2");
        let method = manager.get_input_method(&seat, &handle, ());
        let mut state = MethodState::default();
        queue.roundtrip(&mut state).expect("vaht answers");
        assert!(!state.is_unavailable, "no other input method is in use");
        Self {
            queue,
            state,
            method,
        }
    }

    /// Waits until vaht says a text field is active; returns whether one
    /// came to be.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn wait_until_active(&mut self) -> bool {
        let deadline = Instant::now() + PATIENCE;
        while !self.state.is_active && Instant::now() < deadline {
            self.queue.roundtrip(&mut self.state).expect("vaht answers");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.state.is_active
    }

    /// Types `text` into the active text field.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn type_text(&mut self, text: &str) {
        self.method.commit_string(text.to_owned());
        self.method.commit(self.state.done_count);
        self.queue.roundtrip(&mut self.state).expect("vaht answers");
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for MethodState {
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

impl Dispatch<wl_seat::WlSeat, ()> for MethodState {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpInputMethodManagerV2, ()> for MethodState {
    fn event(
        _: &mut Self,
        _: &ZwpInputMethodManagerV2,
        _: <ZwpInputMethodManagerV2 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpInputMethodV2, ()> for MethodState {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::Activate => state.pending_active = true,
            zwp_input_method_v2::Event::Deactivate => state.pending_active = false,
            zwp_input_method_v2::Event::Done => {
                state.is_active = state.pending_active;
                state.done_count += 1;
            }
            zwp_input_method_v2::Event::Unavailable => state.is_unavailable = true,
            zwp_input_method_v2::Event::SurroundingText { .. }
            | zwp_input_method_v2::Event::TextChangeCause { .. }
            | zwp_input_method_v2::Event::ContentType { .. }
            | _ => {}
        }
    }
}

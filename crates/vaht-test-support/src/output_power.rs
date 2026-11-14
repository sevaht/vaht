//! A program turning an output off and on (`zwlr_output_power_v1`), as
//! `wlopm` does, as tests drive it.

use std::path::Path;
use std::time::Instant;

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_registry};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use smithay_client_toolkit::reexports::protocols_wlr::output_power_management::v1::client::zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1;
use smithay_client_toolkit::reexports::protocols_wlr::output_power_management::v1::client::zwlr_output_power_v1::{self, Mode, ZwlrOutputPowerV1};

use crate::wait::PATIENCE;

/// A connection controlling the power of vaht's first output.
#[derive(Debug)]
pub struct OutputPowerControl {
    /// The events vaht sends.
    queue: EventQueue<PowerState>,
    /// What they said.
    state: PowerState,
    /// The control.
    control: ZwlrOutputPowerV1,
}

/// What vaht said of the output's power.
#[derive(Debug, Default)]
struct PowerState {
    /// Whether the output is on, once vaht has said.
    is_on: Option<bool>,
}

impl OutputPowerControl {
    /// Connects to vaht at `socket` and controls its first output.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be reached or offers no power control.
    #[must_use]
    pub fn connect(socket: &Path) -> Self {
        let stream =
            std::os::unix::net::UnixStream::connect(socket).expect("vaht's socket accepts");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) =
            registry_queue_init::<PowerState>(&connection).expect("the registry is read");
        let handle = queue.handle();
        let output: wl_output::WlOutput = globals
            .bind(&handle, 1..=4, ())
            .expect("vaht offers an output");
        let manager: ZwlrOutputPowerManagerV1 = globals
            .bind(&handle, 1..=1, ())
            .expect("vaht offers zwlr_output_power_manager_v1");
        let control = manager.get_output_power(&output, &handle, ());
        let mut state = PowerState::default();
        queue.roundtrip(&mut state).expect("vaht answers");
        Self {
            queue,
            state,
            control,
        }
    }

    /// Turns the output on or off (`is_on`).
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn set(&mut self, is_on: bool) {
        self.control
            .set_mode(if is_on { Mode::On } else { Mode::Off });
        self.queue.flush().expect("vaht answers");
    }

    /// Waits until vaht says the output is on, or off (`is_on`); returns
    /// whether it said so.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn wait_for(&mut self, is_on: bool) -> bool {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.queue
                .dispatch_pending(&mut self.state)
                .expect("vaht answers");
            let left = deadline.saturating_duration_since(Instant::now());
            if self.state.is_on == Some(is_on) || left.is_zero() {
                return self.state.is_on == Some(is_on);
            }
            self.queue.flush().expect("vaht answers");
            if let Some(guard) = self.queue.prepare_read() {
                let mut fds = [PollFd::new(guard.connection_fd(), PollFlags::POLLIN)];
                let millis = u16::try_from(left.as_millis()).unwrap_or(u16::MAX);
                if poll(&mut fds, PollTimeout::from(millis)).unwrap_or(0) > 0 {
                    let _ = guard.read();
                }
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for PowerState {
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

impl Dispatch<wl_output::WlOutput, ()> for PowerState {
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

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for PowerState {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputPowerManagerV1,
        _: <ZwlrOutputPowerManagerV1 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrOutputPowerV1, ()> for PowerState {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputPowerV1,
        event: zwlr_output_power_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_power_v1::Event::Mode { mode } => {
                state.is_on = Some(mode == WEnum::Value(Mode::On));
            }
            zwlr_output_power_v1::Event::Failed => state.is_on = None,
            _ => {}
        }
    }
}

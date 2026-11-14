//! A program arranging outputs (`zwlr_output_manager_v1`), as wlr-randr
//! and kanshi do, as tests drive it.

use std::path::Path;
use std::time::Instant;

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::wl_registry;
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, EventQueue, QueueHandle, event_created_child};
use smithay_client_toolkit::reexports::protocols_wlr::output_management::v1::client::zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1;
use smithay_client_toolkit::reexports::protocols_wlr::output_management::v1::client::zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1};
use smithay_client_toolkit::reexports::protocols_wlr::output_management::v1::client::zwlr_output_head_v1::{self, ZwlrOutputHeadV1};
use smithay_client_toolkit::reexports::protocols_wlr::output_management::v1::client::zwlr_output_manager_v1::{self, ZwlrOutputManagerV1};
use smithay_client_toolkit::reexports::protocols_wlr::output_management::v1::client::zwlr_output_mode_v1::{self, ZwlrOutputModeV1};

use crate::wait::PATIENCE;

/// An output as the configurator was last told of it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfiguredHead {
    /// Its connector.
    pub name: String,
    /// Whether it is on.
    pub is_enabled: bool,
    /// Where its corner is.
    pub position: (i32, i32),
    /// Its current mode's size, while on.
    pub size: Option<(i32, i32)>,
}

/// What vaht said of a configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigurationResult {
    /// It was carried out (or, tested, could be).
    Succeeded,
    /// It could not be.
    Failed,
    /// The outputs changed since it was made.
    Cancelled,
}

/// What a configuration asks of one output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadChange {
    /// On, its corner here.
    At(i32, i32),
    /// Off.
    Off,
}

/// A connection arranging vaht's outputs.
#[derive(Debug)]
pub struct OutputConfigurator {
    /// The events vaht sends.
    queue: EventQueue<ConfiguratorState>,
    /// What they said.
    state: ConfiguratorState,
    /// The manager.
    manager: ZwlrOutputManagerV1,
}

/// A head as its events say, and its modes with their sizes.
type KnownHead = (
    ZwlrOutputHeadV1,
    ConfiguredHead,
    Vec<(ZwlrOutputModeV1, (i32, i32))>,
);

/// What vaht's events said.
#[derive(Debug, Default)]
struct ConfiguratorState {
    /// Each head, as its events since say, and its modes with their sizes.
    heads: Vec<KnownHead>,
    /// The serial of the last `done`.
    serial: Option<u32>,
    /// What vaht said of the last configuration.
    result: Option<ConfigurationResult>,
}

impl OutputConfigurator {
    /// Connects to vaht at `socket` and reads its outputs.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be reached or offers no output manager.
    #[must_use]
    pub fn connect(socket: &Path) -> Self {
        let stream =
            std::os::unix::net::UnixStream::connect(socket).expect("vaht's socket accepts");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) =
            registry_queue_init::<ConfiguratorState>(&connection).expect("the registry is read");
        let manager: ZwlrOutputManagerV1 = globals
            .bind(&queue.handle(), 4..=4, ())
            .expect("vaht offers zwlr_output_manager_v1");
        let mut state = ConfiguratorState::default();
        queue.roundtrip(&mut state).expect("vaht answers");
        Self {
            queue,
            state,
            manager,
        }
    }

    /// Waits until the outputs are as `is_done` wants them; returns them
    /// as they then are.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn wait_for(&mut self, is_done: impl Fn(&[ConfiguredHead]) -> bool) -> Vec<ConfiguredHead> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.dispatch_until(deadline);
            let heads = self.heads();
            if is_done(&heads) || Instant::now() >= deadline {
                return heads;
            }
        }
    }

    /// Asks for `changes` (each output by connector) under the serial of
    /// the outputs as last told, or `serial` if given; returns what vaht
    /// said. Outputs `changes` leaves out stay as they are.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn apply(
        &mut self,
        changes: &[(&str, HeadChange)],
        serial: Option<u32>,
    ) -> Option<ConfigurationResult> {
        let handle = self.queue.handle();
        let serial = serial.or(self.state.serial).unwrap_or_default();
        let configuration = self.manager.create_configuration(serial, &handle, ());
        for (head, known, _) in &self.state.heads {
            let wanted = changes
                .iter()
                .find(|(name, _)| *name == known.name)
                .map(|(_, change)| *change);
            match (wanted, known.is_enabled) {
                (Some(HeadChange::At(x, y)), _) => {
                    configuration
                        .enable_head(head, &handle, ())
                        .set_position(x, y);
                }
                (None, true) => {
                    configuration.enable_head(head, &handle, ());
                }
                (Some(HeadChange::Off), _) | (None, false) => configuration.disable_head(head),
            }
        }
        self.state.result = None;
        configuration.apply();
        let deadline = Instant::now() + PATIENCE;
        while self.state.result.is_none() && Instant::now() < deadline {
            self.dispatch_until(deadline);
        }
        configuration.destroy();
        self.state.result
    }

    /// Returns the serial of the outputs as last told.
    #[must_use]
    pub const fn serial(&self) -> Option<u32> {
        self.state.serial
    }

    /// Returns the heads as their events say.
    fn heads(&self) -> Vec<ConfiguredHead> {
        self.state
            .heads
            .iter()
            .map(|(_, head, _)| head.clone())
            .collect()
    }

    /// Reads what vaht sent, waiting for it until `deadline`.
    fn dispatch_until(&mut self, deadline: Instant) {
        self.queue
            .dispatch_pending(&mut self.state)
            .expect("vaht answers");
        self.queue.flush().expect("vaht answers");
        let left = deadline.saturating_duration_since(Instant::now());
        if let Some(guard) = self.queue.prepare_read() {
            let mut fds = [PollFd::new(guard.connection_fd(), PollFlags::POLLIN)];
            let millis = u16::try_from(left.as_millis().min(50)).unwrap_or(50);
            if poll(&mut fds, PollTimeout::from(millis)).unwrap_or(0) > 0 {
                let _ = guard.read();
            }
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .expect("vaht answers");
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ConfiguratorState {
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

impl Dispatch<ZwlrOutputManagerV1, ()> for ConfiguratorState {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                state
                    .heads
                    .push((head, ConfiguredHead::default(), Vec::new()));
            }
            zwlr_output_manager_v1::Event::Done { serial } => state.serial = Some(serial),
            zwlr_output_manager_v1::Event::Finished | _ => {}
        }
    }

    event_created_child!(ConfiguratorState, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for ConfiguratorState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some((_, head, modes)) = state.heads.iter_mut().find(|(known, ..)| known == proxy)
        else {
            return;
        };
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "the rest of a head's description needs nothing here"
        )]
        match event {
            zwlr_output_head_v1::Event::Name { name } => head.name = name,
            zwlr_output_head_v1::Event::Enabled { enabled } => {
                head.is_enabled = enabled != 0;
                if !head.is_enabled {
                    head.size = None;
                }
            }
            zwlr_output_head_v1::Event::Position { x, y } => head.position = (x, y),
            zwlr_output_head_v1::Event::Mode { mode } => modes.push((mode, (0, 0))),
            zwlr_output_head_v1::Event::CurrentMode { mode } => {
                head.size = modes
                    .iter()
                    .find(|(known, _)| *known == mode)
                    .map(|(_, size)| *size);
            }
            zwlr_output_head_v1::Event::Finished => {
                state.heads.retain(|(known, ..)| known != proxy);
            }
            _ => {}
        }
    }

    event_created_child!(ConfiguratorState, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for ConfiguratorState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_output_mode_v1::Event::Size { width, height } = event {
            for (_, _, modes) in &mut state.heads {
                if let Some((_, size)) = modes.iter_mut().find(|(known, _)| known == proxy) {
                    *size = (width, height);
                }
            }
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for ConfiguratorState {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.result = match event {
            zwlr_output_configuration_v1::Event::Succeeded => Some(ConfigurationResult::Succeeded),
            zwlr_output_configuration_v1::Event::Failed => Some(ConfigurationResult::Failed),
            zwlr_output_configuration_v1::Event::Cancelled => Some(ConfigurationResult::Cancelled),
            _ => state.result,
        };
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ()> for ConfiguratorState {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputConfigurationHeadV1,
        _: <ZwlrOutputConfigurationHeadV1 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

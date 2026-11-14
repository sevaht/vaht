//! A taskbar outside vaht (`zwlr_foreign_toplevel_manager_v1`), as tests
//! drive it: waybar's taskbar sees and works the windows this way.
//!
//! [`ToplevelManager`] connects on its own, hears of each window's title
//! and state, and activates, minimizes, maximizes or closes windows.

use std::path::Path;
use std::time::Instant;

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_registry, wl_seat};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, EventQueue, QueueHandle, event_created_child};
use smithay_client_toolkit::reexports::protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1};
use smithay_client_toolkit::reexports::protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1};

use crate::wait::PATIENCE;

/// A window as the manager knows it: its title and its states
/// (`maximized`, `minimized`, `activated`, `fullscreen`), as its last
/// `done` left them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagedWindow {
    /// Its title.
    pub title: String,
    /// The states that hold, by name, in the protocol's order.
    pub states: Vec<&'static str>,
}

/// A connection managing vaht's windows.
#[derive(Debug)]
pub struct ToplevelManager {
    /// The events vaht sends.
    queue: EventQueue<ManagerState>,
    /// What they said.
    state: ManagerState,
    /// The seat, which activation names.
    seat: wl_seat::WlSeat,
    /// The manager, held while windows are managed.
    _manager: ZwlrForeignToplevelManagerV1,
}

/// What vaht's events said.
#[derive(Debug, Default)]
struct ManagerState {
    /// Each window's handle, what its last `done` said, and what its
    /// events since say, in the order they were announced.
    windows: Vec<(
        ZwlrForeignToplevelHandleV1,
        Option<ManagedWindow>,
        ManagedWindow,
    )>,
}

impl ToplevelManager {
    /// Connects to vaht at `socket` and starts managing its windows.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be reached or offers no manager.
    #[must_use]
    pub fn connect(socket: &Path) -> Self {
        let stream =
            std::os::unix::net::UnixStream::connect(socket).expect("vaht's socket accepts");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) =
            registry_queue_init::<ManagerState>(&connection).expect("the registry is read");
        let handle = queue.handle();
        let seat: wl_seat::WlSeat = globals
            .bind(&handle, 1..=9, ())
            .expect("vaht offers wl_seat");
        let manager: ZwlrForeignToplevelManagerV1 = globals
            .bind(&handle, 3..=3, ())
            .expect("vaht offers zwlr_foreign_toplevel_manager_v1");
        let mut state = ManagerState::default();
        queue.roundtrip(&mut state).expect("vaht answers");
        Self {
            queue,
            state,
            seat,
            _manager: manager,
        }
    }

    /// Waits until the windows, in the order announced, are as `is_done`
    /// wants them; returns them as they then are.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn wait_for(&mut self, is_done: impl Fn(&[ManagedWindow]) -> bool) -> Vec<ManagedWindow> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.queue
                .dispatch_pending(&mut self.state)
                .expect("vaht answers");
            let windows = self.state.shown();
            let left = deadline.saturating_duration_since(Instant::now());
            if is_done(&windows) || left.is_zero() {
                return windows;
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

    /// Brings window `index` (in the order announced) forward with focus.
    pub fn activate(&mut self, index: usize) {
        let handle = self.handle(index);
        handle.activate(&self.seat);
        self.flush();
    }

    /// Minimizes window `index`.
    pub fn minimize(&mut self, index: usize) {
        self.handle(index).set_minimized();
        self.flush();
    }

    /// Maximizes window `index`.
    pub fn maximize(&mut self, index: usize) {
        self.handle(index).set_maximized();
        self.flush();
    }

    /// Asks window `index` to close.
    pub fn close(&mut self, index: usize) {
        self.handle(index).close();
        self.flush();
    }

    /// Returns the handle of window `index`.
    fn handle(&self, index: usize) -> ZwlrForeignToplevelHandleV1 {
        self.state.windows[index].0.clone()
    }

    /// Sends what was asked.
    fn flush(&self) {
        self.queue.flush().expect("vaht answers");
    }
}

impl ManagerState {
    /// Returns the windows as their last `done` left them.
    fn shown(&self) -> Vec<ManagedWindow> {
        self.windows
            .iter()
            .filter_map(|(_, shown, _)| shown.clone())
            .collect()
    }

    /// Returns what the events since `handle`'s last `done` say.
    fn pending_of(&mut self, handle: &ZwlrForeignToplevelHandleV1) -> Option<&mut ManagedWindow> {
        self.windows
            .iter_mut()
            .find(|(known, ..)| known == handle)
            .map(|(_, _, pending)| pending)
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ManagerState {
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

impl Dispatch<wl_seat::WlSeat, ()> for ManagerState {
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

impl Dispatch<wl_output::WlOutput, ()> for ManagerState {
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

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for ManagerState {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state
                .windows
                .push((toplevel, None, ManagedWindow::default()));
        }
    }

    event_created_child!(ManagerState, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for ManagerState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "the program, outputs and parent need nothing here"
        )]
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                if let Some(pending) = state.pending_of(proxy) {
                    pending.title = title;
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::State { state: numbers } => {
                const NAMES: [&str; 4] = ["maximized", "minimized", "activated", "fullscreen"];
                if let Some(pending) = state.pending_of(proxy) {
                    pending.states = numbers
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .filter_map(|bytes| {
                            let number = u32::from_ne_bytes(*bytes);
                            NAMES.get(usize::try_from(number).ok()?).copied()
                        })
                        .collect();
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Done => {
                if let Some((_, shown, pending)) =
                    state.windows.iter_mut().find(|(known, ..)| known == proxy)
                {
                    *shown = Some(pending.clone());
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                state.windows.retain(|(known, ..)| known != proxy);
            }
            _ => {}
        }
    }
}

//! A dock's view of the windows (`ext_foreign_toplevel_list_v1`), as tests
//! watch it.
//!
//! [`WindowList`] connects on its own and hears of each window opened,
//! renamed or closed, as docks and window switchers do.

use std::path::Path;
use std::time::Instant;

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use smithay_client_toolkit::reexports::client::backend::ObjectId;
use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::wl_registry;
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, EventQueue, Proxy as _, QueueHandle, event_created_child};
use smithay_client_toolkit::reexports::protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1};
use smithay_client_toolkit::reexports::protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1};

use crate::wait::PATIENCE;

/// A window as the list shows it: its title and its program's app id.
pub type ListedWindow = (String, String);

/// A connection watching vaht's list of windows.
#[derive(Debug)]
pub struct WindowList {
    /// The events vaht sends.
    queue: EventQueue<ListState>,
    /// What they said.
    state: ListState,
    /// The list, held while watched.
    _list: ExtForeignToplevelListV1,
}

/// What vaht's events said.
#[derive(Debug, Default)]
struct ListState {
    /// Each window listed, in the order listed: its handle, and what its
    /// last `done` said, once it has.
    windows: Vec<(ObjectId, Option<ListedWindow>)>,
    /// Each window's title and app id since its last `done`.
    pending: Vec<(ObjectId, ListedWindow)>,
}

impl WindowList {
    /// Connects to vaht at `socket` and starts watching its windows.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be reached or lists no windows.
    #[must_use]
    pub fn connect(socket: &Path) -> Self {
        let stream =
            std::os::unix::net::UnixStream::connect(socket).expect("vaht's socket accepts");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) =
            registry_queue_init::<ListState>(&connection).expect("the registry is read");
        let list: ExtForeignToplevelListV1 = globals
            .bind(&queue.handle(), 1..=1, ())
            .expect("vaht offers ext_foreign_toplevel_list_v1");
        let mut state = ListState::default();
        queue.roundtrip(&mut state).expect("vaht answers");
        Self {
            queue,
            state,
            _list: list,
        }
    }

    /// Returns the windows listed now, in the order they were listed.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn windows(&mut self) -> Vec<ListedWindow> {
        self.queue.roundtrip(&mut self.state).expect("vaht answers");
        self.state.listed()
    }

    /// Waits until the windows listed are `expected`; returns whether they
    /// came to be.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn wait_for(&mut self, expected: &[(&str, &str)]) -> bool {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.queue
                .dispatch_pending(&mut self.state)
                .expect("vaht answers");
            let listed = self.state.listed();
            let is_expected = listed.len() == expected.len()
                && listed.iter().zip(expected).all(
                    |((title, app_id), (want_title, want_app_id))| {
                        title == want_title && app_id == want_app_id
                    },
                );
            if is_expected {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
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

impl ListState {
    /// Returns the windows listed, as their last `done` left them.
    fn listed(&self) -> Vec<ListedWindow> {
        self.windows
            .iter()
            .filter_map(|(_, shown)| shown.clone())
            .collect()
    }

    /// Returns the window of `handle` as its events since its last `done`
    /// say, made the first time.
    fn pending_of(&mut self, handle: &ExtForeignToplevelHandleV1) -> &mut ListedWindow {
        let id = handle.id();
        let known = self.pending.iter().position(|(known, _)| *known == id);
        let index = known.unwrap_or_else(|| {
            self.pending.push((id, ListedWindow::default()));
            self.pending.len() - 1
        });
        &mut self.pending[index].1
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ListState {
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

impl Dispatch<ExtForeignToplevelListV1, ()> for ListState {
    fn event(
        state: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.windows.push((toplevel.id(), None));
        }
    }

    event_created_child!(ListState, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for ListState {
    fn event(
        state: &mut Self,
        proxy: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "a window's identifier, and events of later versions, need nothing"
        )]
        match event {
            ext_foreign_toplevel_handle_v1::Event::Title { title } => {
                state.pending_of(proxy).0 = title;
            }
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                state.pending_of(proxy).1 = app_id;
            }
            ext_foreign_toplevel_handle_v1::Event::Done => {
                let window = state.pending_of(proxy).clone();
                let id = proxy.id();
                if let Some((_, shown)) = state.windows.iter_mut().find(|(known, _)| *known == id) {
                    *shown = Some(window);
                }
            }
            ext_foreign_toplevel_handle_v1::Event::Closed => {
                let id = proxy.id();
                state.windows.retain(|(known, _)| *known != id);
                state.pending.retain(|(known, _)| *known != id);
                proxy.destroy();
            }
            _ => {}
        }
    }
}

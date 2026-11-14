//! A pager outside vaht (`ext_workspace_manager_v1`), as waybar's
//! workspaces module is, as tests drive it.

use std::path::Path;
use std::time::Instant;

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::wl_registry;
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, event_created_child};
use smithay_client_toolkit::reexports::protocols::ext::workspace::v1::client::ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1};
use smithay_client_toolkit::reexports::protocols::ext::workspace::v1::client::ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1};
use smithay_client_toolkit::reexports::protocols::ext::workspace::v1::client::ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1};

use crate::wait::PATIENCE;

/// A workspace as the pager knows it: its name, and whether it is shown.
pub type PagedWorkspace = (String, bool);

/// A connection watching and switching vaht's workspaces.
#[derive(Debug)]
pub struct Pager {
    /// The events vaht sends.
    queue: EventQueue<PagerState>,
    /// What they said.
    state: PagerState,
    /// The manager.
    manager: ExtWorkspaceManagerV1,
}

/// What vaht's events said.
#[derive(Debug, Default)]
struct PagerState {
    /// The group, once sent.
    group: Option<ExtWorkspaceGroupHandleV1>,
    /// Each workspace's handle and what its events said, in the order sent.
    workspaces: Vec<(ExtWorkspaceHandleV1, PagedWorkspace)>,
}

impl Pager {
    /// Connects to vaht at `socket` and starts watching its workspaces.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be reached or offers no workspace manager.
    #[must_use]
    pub fn connect(socket: &Path) -> Self {
        let stream =
            std::os::unix::net::UnixStream::connect(socket).expect("vaht's socket accepts");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) =
            registry_queue_init::<PagerState>(&connection).expect("the registry is read");
        let manager: ExtWorkspaceManagerV1 = globals
            .bind(&queue.handle(), 1..=1, ())
            .expect("vaht offers ext_workspace_manager_v1");
        let mut state = PagerState::default();
        queue.roundtrip(&mut state).expect("vaht answers");
        Self {
            queue,
            state,
            manager,
        }
    }

    /// Waits until the workspaces are as `is_done` wants them; returns them
    /// as they then are.
    ///
    /// # Panics
    ///
    /// Panics if the connection to vaht fails.
    pub fn wait_for(&mut self, is_done: impl Fn(&[PagedWorkspace]) -> bool) -> Vec<PagedWorkspace> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.queue
                .dispatch_pending(&mut self.state)
                .expect("vaht answers");
            let workspaces: Vec<PagedWorkspace> = self
                .state
                .workspaces
                .iter()
                .map(|(_, workspace)| workspace.clone())
                .collect();
            let left = deadline.saturating_duration_since(Instant::now());
            if is_done(&workspaces) || left.is_zero() {
                return workspaces;
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

    /// Shows workspace `index`, in the order sent.
    pub fn activate(&mut self, index: usize) {
        self.state.workspaces[index].0.activate();
        self.commit();
    }

    /// Removes workspace `index`.
    pub fn remove(&mut self, index: usize) {
        self.state.workspaces[index].0.remove();
        self.commit();
    }

    /// Makes a workspace named `name`.
    ///
    /// # Panics
    ///
    /// Panics if vaht sent no group.
    pub fn create(&mut self, name: &str) {
        self.state
            .group
            .as_ref()
            .expect("vaht sent a group")
            .create_workspace(name.to_owned());
        self.commit();
    }

    /// Commits what was asked.
    fn commit(&self) {
        self.manager.commit();
        self.queue.flush().expect("vaht answers");
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for PagerState {
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

impl Dispatch<ExtWorkspaceManagerV1, ()> for PagerState {
    fn event(
        state: &mut Self,
        _: &ExtWorkspaceManagerV1,
        event: ext_workspace_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "done and finished need nothing here"
        )]
        match event {
            ext_workspace_manager_v1::Event::WorkspaceGroup { workspace_group } => {
                state.group = Some(workspace_group);
            }
            ext_workspace_manager_v1::Event::Workspace { workspace } => {
                state.workspaces.push((workspace, (String::new(), false)));
            }
            _ => {}
        }
    }

    event_created_child!(PagerState, ExtWorkspaceManagerV1, [
        ext_workspace_manager_v1::EVT_WORKSPACE_GROUP_OPCODE => (ExtWorkspaceGroupHandleV1, ()),
        ext_workspace_manager_v1::EVT_WORKSPACE_OPCODE => (ExtWorkspaceHandleV1, ()),
    ]);
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for PagerState {
    fn event(
        _: &mut Self,
        _: &ExtWorkspaceGroupHandleV1,
        _: ext_workspace_group_handle_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtWorkspaceHandleV1, ()> for PagerState {
    fn event(
        state: &mut Self,
        proxy: &ExtWorkspaceHandleV1,
        event: ext_workspace_handle_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "the id, coordinates and capabilities need nothing here"
        )]
        match event {
            ext_workspace_handle_v1::Event::Name { name } => {
                if let Some((_, workspace)) = state
                    .workspaces
                    .iter_mut()
                    .find(|(known, _)| known == proxy)
                {
                    workspace.0 = name;
                }
            }
            ext_workspace_handle_v1::Event::State { state: shown } => {
                let is_active = matches!(
                    shown,
                    WEnum::Value(flags) if flags.contains(ext_workspace_handle_v1::State::Active)
                );
                if let Some((_, workspace)) = state
                    .workspaces
                    .iter_mut()
                    .find(|(known, _)| known == proxy)
                {
                    workspace.1 = is_active;
                }
            }
            ext_workspace_handle_v1::Event::Removed => {
                state.workspaces.retain(|(known, _)| known != proxy);
            }
            _ => {}
        }
    }
}

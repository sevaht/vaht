//! The workspaces as pagers outside vaht see and switch them
//! (`ext_workspace_manager_v1`): waybar's workspaces module, and the like.
//!
//! vaht's workspaces are shared by every output, as Fluxbox's are, so each
//! program binding the manager gets one group, on every output, holding a
//! handle per workspace: its number, name, place in order and whether it
//! is shown. Any workspace can be activated; only the last can be removed,
//! as vaht removes workspaces from the end; and the group can make a
//! workspace, added at the end. Requests wait for the program's commit,
//! then go to the window manager ([`WorkspaceRequest`]). What changed is
//! decided from plain values ([`WorkspaceFacts`]).

use smithay::output::Output;
use smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1};
use smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1};
use smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::ExtWorkspaceManagerV1;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};

use crate::Compositor;

/// What a pager is told of one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceFacts {
    /// Its name.
    pub(crate) name: String,
    /// Whether it is shown.
    pub(crate) is_active: bool,
    /// Whether it is the last, the only one that can be removed.
    pub(crate) is_last: bool,
}

impl WorkspaceFacts {
    /// Returns what a workspace with these facts may be asked.
    fn capabilities(&self) -> ext_workspace_handle_v1::WorkspaceCapabilities {
        use ext_workspace_handle_v1::WorkspaceCapabilities;
        if self.is_last {
            WorkspaceCapabilities::Activate | WorkspaceCapabilities::Remove
        } else {
            WorkspaceCapabilities::Activate
        }
    }

    /// Returns its state.
    fn state(&self) -> ext_workspace_handle_v1::State {
        if self.is_active {
            ext_workspace_handle_v1::State::Active
        } else {
            ext_workspace_handle_v1::State::empty()
        }
    }
}

/// What a pager asked, carried out at its commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceRequest {
    /// Show the workspace with this index, from 0.
    Activate(usize),
    /// Remove the workspace with this index, if it is the last.
    Remove(usize),
    /// Add a workspace with this name at the end.
    Create(String),
}

/// What a workspace handle stands for: its manager, and its index.
#[derive(Debug, Clone)]
pub(crate) struct HandleData {
    /// The manager it was sent by.
    pub(crate) manager: ExtWorkspaceManagerV1,
    /// The workspace's index, from 0.
    pub(crate) index: usize,
}

/// A manager bound, its group and handles, and what it asked.
#[derive(Debug)]
struct Bound {
    /// The manager.
    manager: ExtWorkspaceManagerV1,
    /// Its group: every workspace, on every output.
    group: ExtWorkspaceGroupHandleV1,
    /// Each workspace's handle, by index.
    handles: Vec<ExtWorkspaceHandleV1>,
    /// The outputs the group was said to be on.
    outputs: Vec<Output>,
    /// What it asked since its last commit.
    pending: Vec<WorkspaceRequest>,
}

/// The managers bound, and the workspaces as last told.
#[derive(Debug)]
pub(crate) struct WorkspacePager {
    /// The global, held to keep it offered.
    _global: GlobalId,
    /// Each manager bound and not stopped.
    managers: Vec<Bound>,
    /// Each workspace, by index, as last told.
    told: Vec<WorkspaceFacts>,
    /// The outputs on, as last told.
    outputs: Vec<Output>,
}

impl WorkspacePager {
    /// The protocol version vaht serves.
    pub(crate) const VERSION: u32 = 1;

    /// Offers the global on `display`.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let global =
            display.create_global::<Compositor, ExtWorkspaceManagerV1, _>(Self::VERSION, ());
        Self {
            _global: global,
            managers: Vec::new(),
            told: Vec::new(),
            outputs: Vec::new(),
        }
    }

    /// Follows `manager`, just bound: it is sent the group and every
    /// workspace.
    pub(crate) fn bind(&mut self, manager: ExtWorkspaceManagerV1, display: &DisplayHandle) {
        let Some(client) = manager.client() else {
            return;
        };
        let Ok(group) = client
            .create_resource::<ExtWorkspaceGroupHandleV1, ExtWorkspaceManagerV1, Compositor>(
                display,
                manager.version(),
                manager.clone(),
            )
        else {
            return;
        };
        manager.workspace_group(&group);
        group.capabilities(ext_workspace_group_handle_v1::GroupCapabilities::CreateWorkspace);
        let mut bound = Bound {
            manager,
            group,
            handles: Vec::new(),
            outputs: Vec::new(),
            pending: Vec::new(),
        };
        Self::follow_outputs(&mut bound, &self.outputs);
        for (index, facts) in self.told.iter().enumerate() {
            Self::announce(&mut bound, index, facts, display);
        }
        bound.manager.done();
        self.managers.push(bound);
    }

    /// Forgets `manager`, which stopped or is gone; it is told it is
    /// finished if `is_stopped`.
    pub(crate) fn forget(&mut self, manager: &ExtWorkspaceManagerV1, is_stopped: bool) {
        self.managers.retain(|bound| bound.manager != *manager);
        if is_stopped {
            manager.finished();
        }
    }

    /// Notes `request` from `manager`'s program, to carry out at its
    /// commit.
    pub(crate) fn ask(&mut self, manager: &ExtWorkspaceManagerV1, request: WorkspaceRequest) {
        if let Some(bound) = self
            .managers
            .iter_mut()
            .find(|bound| bound.manager == *manager)
        {
            bound.pending.push(request);
        }
    }

    /// Returns what `manager`'s program asked since its last commit, which
    /// it just made.
    pub(crate) fn commit(&mut self, manager: &ExtWorkspaceManagerV1) -> Vec<WorkspaceRequest> {
        self.managers
            .iter_mut()
            .find(|bound| bound.manager == *manager)
            .map(|bound| std::mem::take(&mut bound.pending))
            .unwrap_or_default()
    }

    /// Follows the workspaces, `workspaces` by index, and the outputs on,
    /// `outputs`: if anything changed, each manager is told.
    pub(crate) fn follow(
        &mut self,
        workspaces: Vec<WorkspaceFacts>,
        outputs: Vec<Output>,
        display: &DisplayHandle,
    ) {
        if workspaces == self.told && outputs == self.outputs {
            return;
        }
        for bound in &mut self.managers {
            Self::follow_outputs(bound, &outputs);
            while bound.handles.len() > workspaces.len() {
                if let Some(handle) = bound.handles.pop() {
                    bound.group.workspace_leave(&handle);
                    handle.removed();
                }
            }
            for (index, facts) in workspaces.iter().enumerate() {
                match (bound.handles.get(index), self.told.get(index)) {
                    (Some(handle), Some(before)) if before != facts => Self::tell(handle, facts),
                    (Some(_), _) => {}
                    (None, _) => Self::announce(bound, index, facts, display),
                }
            }
            bound.manager.done();
        }
        self.told = workspaces;
        self.outputs = outputs;
    }

    /// Tells `bound`'s group which of `outputs` it is on now.
    fn follow_outputs(bound: &mut Bound, outputs: &[Output]) {
        let Some(client) = bound.manager.client() else {
            return;
        };
        for output in bound
            .outputs
            .iter()
            .filter(|output| !outputs.contains(output))
        {
            for wl_output in output.client_outputs(&client) {
                bound.group.output_leave(&wl_output);
            }
        }
        for output in outputs
            .iter()
            .filter(|output| !bound.outputs.contains(output))
        {
            for wl_output in output.client_outputs(&client) {
                bound.group.output_enter(&wl_output);
            }
        }
        bound.outputs = outputs.to_vec();
    }

    /// Sends `bound` a handle for workspace `index` with `facts`, in its
    /// group.
    fn announce(bound: &mut Bound, index: usize, facts: &WorkspaceFacts, display: &DisplayHandle) {
        let Some(client) = bound.manager.client() else {
            return;
        };
        let data = HandleData {
            manager: bound.manager.clone(),
            index,
        };
        let Ok(handle) = client.create_resource::<ExtWorkspaceHandleV1, HandleData, Compositor>(
            display,
            bound.manager.version(),
            data,
        ) else {
            return;
        };
        bound.manager.workspace(&handle);
        handle.id((index + 1).to_string());
        let coordinate = u32::try_from(index).unwrap_or(u32::MAX);
        handle.coordinates(coordinate.to_ne_bytes().to_vec());
        Self::tell(&handle, facts);
        bound.group.workspace_enter(&handle);
        bound.handles.push(handle);
    }

    /// Tells `handle`'s program `facts`.
    fn tell(handle: &ExtWorkspaceHandleV1, facts: &WorkspaceFacts) {
        handle.name(facts.name.clone());
        handle.state(facts.state());
        handle.capabilities(facts.capabilities());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_workspace_can_be_removed_and_the_shown_one_is_active() {
        use ext_workspace_handle_v1::{State, WorkspaceCapabilities};

        let facts = |is_active, is_last| WorkspaceFacts {
            name: String::new(),
            is_active,
            is_last,
        };
        assert_eq!(
            facts(false, false).capabilities(),
            WorkspaceCapabilities::Activate
        );
        assert!(
            facts(false, true)
                .capabilities()
                .contains(WorkspaceCapabilities::Remove)
        );
        assert_eq!(facts(true, false).state(), State::Active);
        assert_eq!(facts(false, false).state(), State::empty());
    }
}

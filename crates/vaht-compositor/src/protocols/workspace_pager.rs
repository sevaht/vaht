//! `ext_workspace_manager_v1`, its group and workspace handles: pagers
//! outside vaht ([`crate::workspace_pager`]).

use smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1};
use smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1};
use smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};

use crate::Compositor;
use crate::client::ClientState;
use crate::workspace_pager::{HandleData, WorkspaceRequest};

impl GlobalDispatch<ExtWorkspaceManagerV1, ()> for Compositor {
    fn bind(
        state: &mut Self,
        handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtWorkspaceManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        state.protocols.workspace_pager.bind(manager, handle);
    }

    fn can_view(client: Client, (): &()) -> bool {
        ClientState::is_unrestricted(&client)
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ExtWorkspaceManagerV1,
        request: ext_workspace_manager_v1::Request,
        (): &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_workspace_manager_v1::Request::Commit => {
                let asked = state.protocols.workspace_pager.commit(resource);
                state.carry_out_workspace_requests(asked);
            }
            ext_workspace_manager_v1::Request::Stop => {
                state.protocols.workspace_pager.forget(resource, true);
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, resource: &ExtWorkspaceManagerV1, (): &()) {
        state.protocols.workspace_pager.forget(resource, false);
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ExtWorkspaceManagerV1> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ExtWorkspaceGroupHandleV1,
        request: ext_workspace_group_handle_v1::Request,
        data: &ExtWorkspaceManagerV1,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_workspace_group_handle_v1::Request::CreateWorkspace { workspace } = request {
            state
                .protocols
                .workspace_pager
                .ask(data, WorkspaceRequest::Create(workspace));
        }
    }
}

impl Dispatch<ExtWorkspaceHandleV1, HandleData> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ExtWorkspaceHandleV1,
        request: ext_workspace_handle_v1::Request,
        data: &HandleData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use ext_workspace_handle_v1::Request;
        let asked = match request {
            Request::Activate => WorkspaceRequest::Activate(data.index),
            Request::Remove => WorkspaceRequest::Remove(data.index),
            // Workspaces are on every output and shown one at a time: there
            // is nothing to hide one by, nor another group to move it to.
            Request::Deactivate | Request::Assign { .. } | Request::Destroy | _ => return,
        };
        state.protocols.workspace_pager.ask(&data.manager, asked);
    }
}

//! `zwlr_foreign_toplevel_manager_v1` and its handles: what programs
//! outside vaht are told of the windows ([`crate::toplevel_management`]),
//! and what they ask of them, which the compositor carries out.

use smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1};
use smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};

use crate::Compositor;
use crate::toplevel_management::{HandleRequest, ToplevelManagement};
use crate::window_manager::WindowId;

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for Compositor {
    fn bind(
        state: &mut Self,
        handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        state.toplevel_management.bind(manager, handle);
    }

    fn can_view(client: Client, (): &()) -> bool {
        ToplevelManagement::may_bind(&client)
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrForeignToplevelManagerV1,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        (): &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            state.toplevel_management.stop(resource);
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        manager: &ZwlrForeignToplevelManagerV1,
        (): &(),
    ) {
        state.toplevel_management.forget(manager);
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, WindowId> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        data: &WindowId,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Request;
        let asked = match request {
            Request::SetMaximized => HandleRequest::Maximize(true),
            Request::UnsetMaximized => HandleRequest::Maximize(false),
            Request::SetMinimized => HandleRequest::Minimize(true),
            Request::UnsetMinimized => HandleRequest::Minimize(false),
            Request::Activate { .. } => HandleRequest::Activate,
            Request::Close => HandleRequest::Close,
            Request::SetFullscreen { .. } => HandleRequest::Fullscreen(true),
            Request::UnsetFullscreen => HandleRequest::Fullscreen(false),
            // Where a taskbar shows the window is no concern of vaht's;
            // a destroyed handle is simply no longer told anything.
            Request::SetRectangle { .. } | Request::Destroy | _ => return,
        };
        state.carry_out_toplevel_request(*data, &asked);
    }
}

//! `zwlr_gamma_control_manager_v1` and its controls: night-light tools
//! changing an output's colors ([`crate::gamma`]).

use std::fs::File;

use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_manager_v1::{self, ZwlrGammaControlManagerV1};
use smithay::reexports::wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_v1::{self, ZwlrGammaControlV1};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};

use crate::Compositor;
use crate::client::ClientState;
use crate::gamma::GammaRamps;

impl GlobalDispatch<ZwlrGammaControlManagerV1, ()> for Compositor {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrGammaControlManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, (): &()) -> bool {
        ClientState::is_unrestricted(&client)
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrGammaControlManagerV1,
        request: zwlr_gamma_control_manager_v1::Request,
        (): &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_gamma_control_manager_v1::Request::GetGammaControl { id, output } = request {
            let control = data_init.init(id, ());
            match Output::from_resource(&output) {
                Some(output) => {
                    let size = state.backend.gamma_size(&output);
                    state.output_controls.gamma.add(control, output, size);
                }
                None => control.failed(),
            }
        }
    }
}

impl Dispatch<ZwlrGammaControlV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrGammaControlV1,
        request: zwlr_gamma_control_v1::Request,
        (): &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let zwlr_gamma_control_v1::Request::SetGamma { fd } = request else {
            return;
        };
        let Some((output, size)) = state.output_controls.gamma.output_of(resource) else {
            return;
        };
        match GammaRamps::read(File::from(fd), size) {
            Ok(ramps) => state.backend.set_gamma(&output, Some(&ramps)),
            Err(error) => {
                tracing::warn!(%error, output = output.name(), "a program sent unusable gamma ramps");
                if let Some(output) = state.output_controls.gamma.fail(resource) {
                    state.backend.set_gamma(&output, None);
                }
            }
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, resource: &ZwlrGammaControlV1, (): &()) {
        if let Some(output) = state.output_controls.gamma.forget(resource) {
            state.backend.set_gamma(&output, None);
        }
    }
}

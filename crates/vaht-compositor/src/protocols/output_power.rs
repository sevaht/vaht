//! `zwlr_output_power_manager_v1` and its controls: programs turning
//! outputs off and on ([`crate::output_power`]).

use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_manager_v1::{self, ZwlrOutputPowerManagerV1};
use smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_v1::{self, Mode, ZwlrOutputPowerV1};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, WEnum,
};

use crate::Compositor;
use crate::client::ClientState;

impl GlobalDispatch<ZwlrOutputPowerManagerV1, ()> for Compositor {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrOutputPowerManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, (): &()) -> bool {
        ClientState::is_unrestricted(&client)
    }
}

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrOutputPowerManagerV1,
        request: zwlr_output_power_manager_v1::Request,
        (): &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_output_power_manager_v1::Request::GetOutputPower { id, output } = request {
            let output = Output::from_resource(&output);
            let control = data_init.init(id, output.clone());
            match output {
                Some(output) => state.output_power.add_control(control, output),
                None => control.failed(),
            }
        }
    }
}

impl Dispatch<ZwlrOutputPowerV1, Option<Output>> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrOutputPowerV1,
        request: zwlr_output_power_v1::Request,
        data: &Option<Output>,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_output_power_v1::Request::SetMode {
            mode: WEnum::Value(mode),
        } = request
            && let Some(output) = data
        {
            state.set_output_power(output, mode == Mode::On);
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ZwlrOutputPowerV1,
        _data: &Option<Output>,
    ) {
        state.output_power.forget_control(resource);
    }
}

//! `zwlr_output_manager_v1`, its heads and modes, and the configurations
//! programs make with them ([`crate::output_management`]).

use std::sync::{Arc, Mutex};

use smithay::output::{Mode, Output};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_configuration_head_v1::{self, ZwlrOutputConfigurationHeadV1};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_head_v1::{self, ZwlrOutputHeadV1};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_manager_v1::{self, ZwlrOutputManagerV1};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_mode_v1::{self, ZwlrOutputModeV1};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::utils::Transform;

use crate::Compositor;
use crate::client::ClientState;
use crate::output_management::{Configuration, ConfigurationState, HeadRequest};

impl GlobalDispatch<ZwlrOutputManagerV1, ()> for Compositor {
    fn bind(
        state: &mut Self,
        handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrOutputManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        state.output_controls.management.bind(manager, handle);
    }

    fn can_view(client: Client, (): &()) -> bool {
        ClientState::is_unrestricted(&client)
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrOutputManagerV1,
        request: zwlr_output_manager_v1::Request,
        (): &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_output_manager_v1::Request::CreateConfiguration { id, serial } => {
                data_init.init(
                    id,
                    Mutex::new(ConfigurationState {
                        serial,
                        ..ConfigurationState::default()
                    }),
                );
            }
            zwlr_output_manager_v1::Request::Stop => {
                state.output_controls.management.forget(resource, true);
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, resource: &ZwlrOutputManagerV1, (): &()) {
        state.output_controls.management.forget(resource, false);
    }
}

impl Dispatch<ZwlrOutputHeadV1, Output> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ZwlrOutputHeadV1,
        _request: zwlr_output_head_v1::Request,
        _data: &Output,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // Release only destroys the head.
    }
}

impl Dispatch<ZwlrOutputModeV1, Mode> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ZwlrOutputModeV1,
        _request: zwlr_output_mode_v1::Request,
        _data: &Mode,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // Release only destroys the mode.
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, Configuration> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrOutputConfigurationV1,
        request: zwlr_output_configuration_v1::Request,
        data: &Configuration,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_output_configuration_v1::{Error, Request};
        let mut configuration = data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (head, turned_on) = match request {
            Request::EnableHead { id, head } => {
                let asked = Arc::new(Mutex::new(HeadRequest::default()));
                data_init.init(id, Arc::clone(&asked));
                (head, Some(asked))
            }
            Request::DisableHead { head } => (head, None),
            Request::Apply | Request::Test => {
                if configuration.is_used {
                    resource.post_error(Error::AlreadyUsed, "a configuration is used once");
                    return;
                }
                configuration.is_used = true;
                if configuration.serial != state.output_controls.management.serial() {
                    resource.cancelled();
                    return;
                }
                let wanted: Vec<(Output, Option<HeadRequest>)> = configuration
                    .heads
                    .iter()
                    .map(|(output, asked)| {
                        let asked = asked.as_ref().map(|asked| {
                            asked
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .clone()
                        });
                        (output.clone(), asked)
                    })
                    .collect();
                let is_complete = state
                    .outputs
                    .connected()
                    .all(|output| wanted.iter().any(|(named, _)| named == output));
                if !is_complete {
                    resource.post_error(
                        Error::UnconfiguredHead,
                        "every head must be enabled or disabled",
                    );
                    return;
                }
                let is_test = matches!(request, Request::Test);
                if state.apply_output_configuration(&wanted, is_test) {
                    resource.succeeded();
                } else {
                    resource.failed();
                }
                return;
            }
            Request::Destroy | _ => return,
        };
        let Some(output) = head.data::<Output>() else {
            return;
        };
        if configuration.heads.iter().any(|(named, _)| named == output) {
            resource.post_error(Error::AlreadyConfiguredHead, "a head is configured once");
            return;
        }
        configuration.heads.push((output.clone(), turned_on));
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, Arc<Mutex<HeadRequest>>> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ZwlrOutputConfigurationHeadV1,
        request: zwlr_output_configuration_head_v1::Request,
        data: &Arc<Mutex<HeadRequest>>,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_output_configuration_head_v1::Request;
        let mut asked = data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match request {
            Request::SetMode { mode } => {
                if let Some(mode) = mode.data::<Mode>() {
                    asked.mode = Some((mode.size.w, mode.size.h, Some(mode.refresh)));
                }
            }
            Request::SetCustomMode {
                width,
                height,
                refresh,
            } => asked.mode = Some((width, height, (refresh > 0).then_some(refresh))),
            Request::SetPosition { x, y } => asked.position = Some((x, y).into()),
            Request::SetTransform {
                transform: WEnum::Value(transform),
            } => asked.transform = Some(Transform::from(transform)),
            Request::SetScale { scale } => asked.scale = Some(scale),
            // vaht's own `vrr` setting says when refresh varies.
            Request::SetTransform { .. } | Request::SetAdaptiveSync { .. } | _ => {}
        }
    }
}

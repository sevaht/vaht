//! Window activation (`xdg_activation_v1`): a program asking for one of its
//! windows to come forward, with a token proving the user asked.
//!
//! vaht honors a token made by the client that had keyboard focus, used
//! within [`ACTIVATION_PATIENCE`]: the window is restored, its workspace
//! shown, and it is raised and focused. Any other request is refused and
//! logged, so a program in the background cannot take focus by itself.

use std::time::Duration;

use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};

use crate::Compositor;

/// How long after its making a token still activates a window.
const ACTIVATION_PATIENCE: Duration = Duration::from_secs(10);

impl XdgActivationHandler for Compositor {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.protocols.activation
    }

    fn request_activation(
        &mut self,
        _token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        let Some(window) = self.toplevels.window_of(&surface) else {
            return;
        };
        let focused_client = self
            .window_manager
            .focused()
            .and_then(|focused| self.toplevels.get(focused))
            .and_then(|focused| {
                self.display_handle
                    .get_client(focused.wl_surface().id())
                    .ok()
            })
            .map(|client| client.id());
        let is_from_focus = focused_client.is_some() && token_data.client_id == focused_client;
        if is_from_focus && token_data.timestamp.elapsed() < ACTIVATION_PATIENCE {
            tracing::debug!(window = %window, "activation honored");
            self.window_manager.activate(window);
        } else {
            tracing::debug!(window = %window, "activation refused: not asked for by the focused program");
        }
    }
}

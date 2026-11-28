//! Window activation (`xdg_activation_v1`): a program asking for one of its
//! windows to come forward, with a token proving the user asked.
//!
//! vaht honors a token made by the client that had keyboard focus, used
//! within [`ACTIVATION_PATIENCE`]: the window is restored, its workspace
//! shown, and it is raised and focused. Any other request is refused and
//! logged, so a program in the background cannot take focus by itself,
//! unless the program was started by a key or menu item a moment ago (it
//! has the token vaht gave it in `XDG_ACTIVATION_TOKEN`), or a rule says the window may (`focus_stealing = "allow"`), or the
//! user clicked the program's tray item a moment ago
//! (`activation_grace_ms`); a rule saying `"deny"` refuses even a good
//! token.

use std::time::Duration;

use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};

use crate::Compositor;
use crate::config::FocusStealing;
use crate::toplevels::Toplevel;
use crate::window_manager::Window;

/// How long after its making a token still activates a window.
const ACTIVATION_PATIENCE: Duration = Duration::from_secs(10);

/// Marks a token vaht made for a program it started because the user asked
/// (a key or a menu item): any window that program asks to bring forward is
/// brought, as the user just asked for it.
#[derive(Debug)]
struct LaunchToken;

impl Compositor {
    /// Returns a new activation token for a program the user just asked to
    /// start, in the form the program reads from `XDG_ACTIVATION_TOKEN`.
    pub(crate) fn launch_token(&mut self) -> String {
        // Tokens too old to be honored are forgotten.
        self.protocols
            .activation
            .retain_tokens(|_, data| data.timestamp.elapsed() < ACTIVATION_PATIENCE);
        let (token, data) = self.protocols.activation.create_external_token(None);
        data.user_data.insert_if_missing_threadsafe(|| LaunchToken);
        token.as_str().to_owned()
    }
}

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
            .and_then(Toplevel::wl_surface)
            .and_then(|surface| self.display_handle.get_client(surface.id()).ok())
            .map(|client| client.id());
        let is_from_focus = focused_client.is_some() && token_data.client_id == focused_client;
        let is_asked_for = is_from_focus && token_data.timestamp.elapsed() < ACTIVATION_PATIENCE;
        let is_launched = token_data.user_data.get::<LaunchToken>().is_some()
            && token_data.timestamp.elapsed() < ACTIVATION_PATIENCE;
        let policy = self
            .window_manager
            .window(window)
            .map_or(FocusStealing::Default, Window::focus_stealing);
        let grace = self.config.settings.taskbar.tray.activation_grace();
        let is_granted = self.tray_grant.is_some_and(|(pid, at)| {
            at.elapsed() <= grace
                && self
                    .display_handle
                    .get_client(surface.id())
                    .ok()
                    .and_then(|client| client.get_credentials(&self.display_handle).ok())
                    .is_some_and(|credentials| {
                        u32::try_from(credentials.pid).is_ok_and(|client| client == pid)
                    })
        });
        if policy.honors(is_asked_for || is_granted || is_launched) {
            tracing::debug!(window = %window, "activation honored");
            self.window_manager.activate(window);
        } else {
            tracing::debug!(window = %window, "activation refused: not asked for by the focused program");
        }
    }
}

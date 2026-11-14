//! Input methods (`zwp_input_method_v2`) and the text fields they type
//! into (`zwp_text_input_v3`): fcitx5 or ibus composing text, such as
//! Chinese or Japanese, for the focused window.
//!
//! Smithay routes text between the two and follows keyboard focus; vaht
//! shows the input method's candidate popup over the window being typed
//! in, through the same [`PopupManager`] as the window's own popups, and
//! offers it no further.

use smithay::desktop::{PopupKind, PopupManager};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::compositor::with_states;
use smithay::wayland::input_method::{InputMethodHandler, PopupSurface};
use smithay::wayland::shell::xdg::SurfaceCachedState;
use smithay::wayland::text_input::TextInputActivation;

use crate::Compositor;

impl InputMethodHandler for Compositor {
    fn new_popup(&mut self, surface: PopupSurface) {
        if let Err(error) = self.popups.track_popup(PopupKind::from(surface)) {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not show an input method's popup"
            );
        }
    }

    fn dismiss_popup(&mut self, surface: PopupSurface) {
        if let Some(parent) = surface.get_parent().map(|parent| parent.surface.clone()) {
            // A parent already gone took its popups with it.
            let _ = PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
        self.schedule_render();
    }

    fn popup_repositioned(&mut self, _surface: PopupSurface) {
        self.schedule_render();
    }

    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical> {
        with_states(parent, |states| {
            states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .geometry
        })
        .unwrap_or_default()
    }
}

/// A text field's activation needs nothing of vaht: Smithay tells the
/// input method.
impl TextInputActivation for Compositor {}

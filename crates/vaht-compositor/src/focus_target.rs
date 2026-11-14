//! What gets the keyboard: a Wayland surface, or an X11 window.
//!
//! The seat gives keys to a [`FocusTarget`]. A Wayland surface (a window's
//! or a layer surface's) is told through Wayland; an X11 window is also
//! told through X, by Smithay, as its input model asks (`SetInputFocus`,
//! `WM_TAKE_FOCUS`), so X programs take keys as they do on X.

use std::borrow::Cow;

use smithay::backend::input::{InputTime, KeyState};
use smithay::input::Seat;
use smithay::input::keyboard::{KeyboardTarget, KeysymHandle, ModifiersState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Serial};
use smithay::wayland::seat::WaylandFocus;
use smithay::xwayland::X11Surface;

use crate::Compositor;

/// What gets the keyboard.
#[derive(Debug, Clone, PartialEq)]
pub enum FocusTarget {
    /// A Wayland surface: a window's or a layer surface's.
    Surface(WlSurface),
    /// An X11 window, boxed: it is far larger than a surface handle.
    X11(Box<X11Surface>),
}

impl FocusTarget {
    /// Returns the Wayland surface that receives the keys: an X11 window's
    /// is the one Xwayland pairs with it, once it has.
    #[must_use]
    pub fn surface(&self) -> Option<WlSurface> {
        self.wl_surface().map(Cow::into_owned)
    }
}

impl WaylandFocus for FocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Surface(surface) => Some(Cow::Borrowed(surface)),
            Self::X11(window) => window.wl_surface().map(Cow::Owned),
        }
    }
}

impl IsAlive for FocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::Surface(surface) => surface.alive(),
            Self::X11(window) => window.alive(),
        }
    }
}

impl KeyboardTarget<Compositor> for FocusTarget {
    fn enter(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            Self::Surface(surface) => KeyboardTarget::enter(surface, seat, data, keys, serial),
            Self::X11(window) => KeyboardTarget::enter(&**window, seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<Compositor>, data: &mut Compositor, serial: Serial) {
        match self {
            Self::Surface(surface) => KeyboardTarget::leave(surface, seat, data, serial),
            Self::X11(window) => KeyboardTarget::leave(&**window, seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: InputTime,
    ) {
        match self {
            Self::Surface(surface) => {
                KeyboardTarget::key(surface, seat, data, key, state, serial, time);
            }
            Self::X11(window) => {
                KeyboardTarget::key(&**window, seat, data, key, state, serial, time);
            }
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<Compositor>,
        data: &mut Compositor,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            Self::Surface(surface) => {
                KeyboardTarget::modifiers(surface, seat, data, modifiers, serial);
            }
            Self::X11(window) => {
                KeyboardTarget::modifiers(&**window, seat, data, modifiers, serial);
            }
        }
    }
}

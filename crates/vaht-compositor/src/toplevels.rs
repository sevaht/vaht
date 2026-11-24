//! The client's side of every window: the protocol objects behind the
//! window manager's windows.
//!
//! [`Toplevels`] maps each [`WindowId`] the window manager issued to the
//! [`Toplevel`] it stands for. The two are added and removed together, by
//! the compositor, when a client creates or destroys a toplevel; the leak
//! tests check that none outlives its window.

mod toplevel;

use std::collections::HashMap;

use smithay::reexports::wayland_server::Resource as _;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};
use smithay::xwayland::X11Surface;

pub(crate) use self::toplevel::{ParentLink, Toplevel};
use crate::resources::{ResourceKind, ResourceLedger, Tracked};
use crate::window_manager::{WindowId, Workspace};

/// What a surface remembers of the window it had, for a new toplevel its
/// program may give it: Wine destroys and makes the toplevel of a window that
/// gains a caption, and that is the same window to the person using it. The
/// surface is the window's identity; it is forgotten with the surface, not
/// after a length of time.
#[derive(Debug)]
pub(crate) struct Succession {
    /// The surface, which the window was of and the next toplevel is of.
    surface: WlSurface,
    /// Whether the window had the keyboard focus.
    pub(crate) was_focused: bool,
    /// The workspace it was on.
    pub(crate) workspace: Workspace,
    /// The middle of its content.
    pub(crate) center: Point<i32, Logical>,
    /// The window that was directly above it, which the new window goes
    /// below if that is still there: leaving a game does not put it back on
    /// top.
    pub(crate) above: Option<WindowId>,
}

impl Succession {
    /// Returns what the surface `surface` remembers of a window that was on
    /// `workspace`, its content around `center`, and had the focus if
    /// `was_focused`, which has just gone.
    pub(crate) fn new(
        surface: WlSurface,
        was_focused: bool,
        workspace: Workspace,
        center: Point<i32, Logical>,
        above: Option<WindowId>,
    ) -> Self {
        Self {
            surface,
            was_focused,
            workspace,
            center,
            above,
        }
    }

    /// Returns whether a new toplevel of `surface` takes this window's
    /// place.
    fn is_succeeded_by(&self, surface: &WlSurface) -> bool {
        self.surface == *surface
    }

    /// Returns whether the surface lives, so that it may have a new
    /// toplevel.
    fn is_alive(&self) -> bool {
        self.surface.is_alive()
    }
}

/// Every window's toplevel, by window.
#[derive(Debug, Default)]
pub(crate) struct Toplevels {
    /// Each toplevel, with its count, by the window it stands for.
    by_window: HashMap<WindowId, (Toplevel, Tracked)>,
    /// What each surface that lost its toplevel remembers of the window,
    /// until it gets a new one or is destroyed.
    successions: Vec<Succession>,
}

impl Toplevels {
    /// Notes what the surface of the window that was just removed
    /// remembers of it, in case its program gives the surface a new
    /// toplevel; what the surfaces destroyed since remembered is dropped.
    pub(crate) fn expect_successor(&mut self, succession: Option<Succession>) {
        self.successions.retain(Succession::is_alive);
        if let Some(succession) = succession {
            self.successions
                .retain(|known| !known.is_succeeded_by(&succession.surface));
            self.successions.push(succession);
        }
    }

    /// Takes what `surface` remembers of the window it had, if it had one
    /// and a new toplevel of it takes the window's place.
    pub(crate) fn take_succession_of(&mut self, surface: &WlSurface) -> Option<Succession> {
        let position = self
            .successions
            .iter()
            .position(|known| known.is_succeeded_by(surface))?;
        Some(self.successions.swap_remove(position))
    }

    /// Returns the toplevel of `window`, if it has one.
    pub(crate) fn get(&self, window: WindowId) -> Option<&Toplevel> {
        self.by_window.get(&window).map(|(toplevel, _)| toplevel)
    }

    /// Returns the toplevel of `window` to change, if it has one.
    pub(crate) fn get_mut(&mut self, window: WindowId) -> Option<&mut Toplevel> {
        self.by_window
            .get_mut(&window)
            .map(|(toplevel, _)| toplevel)
    }

    /// Returns the window whose root surface is `surface`.
    pub(crate) fn window_of(&self, surface: &WlSurface) -> Option<WindowId> {
        self.by_window
            .iter()
            .find(|(_, (toplevel, _))| toplevel.has_surface(surface))
            .map(|(window, _)| *window)
    }

    /// Returns the window `link` names, if vaht manages it.
    pub(crate) fn window_named(&self, link: &ParentLink) -> Option<WindowId> {
        self.by_window
            .iter()
            .find(|(_, (toplevel, _))| toplevel.is_named_by(link))
            .map(|(window, _)| *window)
    }

    /// Returns the window of the X11 window `surface`, if vaht manages it.
    pub(crate) fn window_of_x11(&self, surface: &X11Surface) -> Option<WindowId> {
        self.by_window
            .iter()
            .find(|(_, (toplevel, _))| toplevel.x11() == Some(surface))
            .map(|(window, _)| *window)
    }

    /// Forgets the toplevel of the X11 window `surface`, which is gone or
    /// unmapped; returns the window it stood for.
    pub(crate) fn remove_x11(&mut self, surface: &X11Surface) -> Option<WindowId> {
        let window = self.window_of_x11(surface)?;
        self.by_window.remove(&window);
        Some(window)
    }

    /// Returns every managed X11 window.
    pub(crate) fn x11_surfaces(&self) -> Vec<X11Surface> {
        self.by_window
            .values()
            .filter_map(|(toplevel, _)| toplevel.x11().cloned())
            .collect()
    }

    /// Returns every toplevel's root surface, shown or not.
    pub(crate) fn surfaces(&self) -> impl Iterator<Item = WlSurface> {
        self.by_window
            .values()
            .filter_map(|(toplevel, _)| toplevel.wl_surface())
    }

    /// Records that `toplevel` stands for `window`.
    pub(crate) fn insert(&mut self, window: WindowId, toplevel: Toplevel, ledger: &ResourceLedger) {
        self.by_window
            .insert(window, (toplevel, ledger.track(ResourceKind::Toplevel)));
    }

    /// Forgets the toplevel whose root surface is `surface`, which is gone;
    /// returns the window it stood for.
    pub(crate) fn remove(&mut self, surface: &WlSurface) -> Option<WindowId> {
        let window = self.window_of(surface)?;
        self.by_window.remove(&window);
        Some(window)
    }
}

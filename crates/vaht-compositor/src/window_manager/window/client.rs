//! The client's side of a window, as the window manager sees it: what the
//! client last committed, and what vaht asks of it.
//!
//! These are plain values. The compositor builds a [`CommittedState`] from
//! the protocol on every commit and carries out each [`RequestedState`] and
//! [`Ending`] the window manager asks for; the window manager never touches
//! a protocol object (M12).

use smithay::utils::{Logical, Rectangle, Size};

use super::placement::Axes;
use super::resize::SizeLimits;

/// What a client last committed for its window.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CommittedState {
    /// The title; empty when the client set none.
    pub(crate) title: String,
    /// The application id (or X11 class); empty when the client set none.
    pub(crate) app_id: String,
    /// The content area within the client's surface: its size, and where
    /// it starts past any shadow the client draws around it.
    pub(crate) geometry: Rectangle<i32, Logical>,
    /// The content sizes the client accepts.
    pub(crate) size_limits: SizeLimits,
}

/// What vaht asks a client to make of its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RequestedState {
    /// The content size asked for; `None` lets the client choose.
    pub(crate) size: Option<Size<i32, Logical>>,
    /// How the window is placed, so the client can drop its shadows and
    /// rounded corners where it touches an edge.
    pub(crate) placement: PlacementKind,
    /// Whether it has keyboard focus, so the client styles itself so.
    pub(crate) is_activated: bool,
    /// Whether the user is resizing it, so the client can redraw cheaply.
    pub(crate) is_resizing: bool,
}

/// How a window is placed, without the areas it returns to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum PlacementKind {
    /// Wherever it was put.
    #[default]
    Floating,
    /// Filling the usable area along the axes.
    Maximized(Axes),
    /// Covering one cell of the usable area.
    Tiled,
    /// Covering the whole output.
    Fullscreen,
}

/// How a window is asked to end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Ending {
    /// Its client is asked to close it, and may ask the user first.
    Close,
    /// Its program is ended at once, as Fluxbox's `Kill` does.
    Kill,
}

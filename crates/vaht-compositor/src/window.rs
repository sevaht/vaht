//! One managed window: its identity, the client's side of it, and where it is.
//!
//! A [`Window`] is owned only by [`Windows`](crate::windows::Windows);
//! everything else refers to it by its [`WindowId`], so a closed window can
//! never be reached through a stale reference (01 §1).

mod frame;
mod toplevel;

use std::fmt;

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};
use vaht_ui::theme::Metrics;

pub(crate) use frame::{Frame, FrameGeometry, FramePart, TitlebarButton, TitlebarHit};
pub(crate) use toplevel::Toplevel;

use crate::resources::{ResourceKind, ResourceLedger, Tracked};

/// Identifies a window for as long as vaht runs; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(u64);

impl WindowId {
    /// Wraps a raw id.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Returns the raw id, as IPC reports it.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for WindowId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A managed window.
#[derive(Debug)]
pub(crate) struct Window {
    /// Its identity.
    id: WindowId,
    /// The client's side: the only part that knows the protocol.
    toplevel: Toplevel,
    /// Where the top-left corner of its content is, in global coordinates.
    location: Point<i32, Logical>,
    /// What vaht draws around it.
    frame: Frame,
    /// Counts the window while it exists.
    _tracked: Tracked,
}

impl Window {
    /// Creates a window for `toplevel` with its content at `location`.
    pub(crate) fn new(
        id: WindowId,
        toplevel: Toplevel,
        location: Point<i32, Logical>,
        ledger: &ResourceLedger,
    ) -> Self {
        Self {
            id,
            toplevel,
            location,
            frame: Frame::new(ledger),
            _tracked: ledger.track(ResourceKind::Window),
        }
    }

    /// Returns the window's identity.
    pub(crate) const fn id(&self) -> WindowId {
        self.id
    }

    /// Returns the client's side of the window.
    pub(crate) const fn toplevel(&self) -> &Toplevel {
        &self.toplevel
    }

    /// Returns where the content's top-left corner is.
    pub(crate) const fn location(&self) -> Point<i32, Logical> {
        self.location
    }

    /// Returns the area the window's content covers, in global coordinates.
    pub(crate) fn content_area(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.location, self.toplevel.geometry().size)
    }

    /// Returns where each part of the frame goes with the theme's `metrics`.
    pub(crate) fn frame_geometry(&self, metrics: Metrics) -> FrameGeometry {
        FrameGeometry::around(self.content_area(), metrics)
    }

    /// Returns the frame.
    pub(crate) const fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Returns the frame, to change it.
    pub(crate) const fn frame_mut(&mut self) -> &mut Frame {
        &mut self.frame
    }

    /// Brings the frame up to date with what the client last committed (its
    /// title); returns whether anything changed.
    pub(crate) fn sync_with_client(&mut self) -> bool {
        let title = self.toplevel.title().unwrap_or_default();
        self.frame.set_title(&title)
    }

    /// Returns where the client's surface is drawn: the content location,
    /// moved by the margin a client-side shadow adds around its content.
    pub(crate) fn surface_origin(&self) -> Point<i32, Logical> {
        self.location - self.toplevel.geometry().loc
    }

    /// Returns the client surface under `point` (global coordinates) and that
    /// surface's origin in global coordinates.
    pub(crate) fn surface_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let origin = self.surface_origin();
        self.toplevel
            .surface_under(point - origin.to_f64())
            .map(|(surface, offset)| (surface, (offset + origin).to_f64()))
    }
}

//! One managed window as the window manager sees it: what its client
//! committed, where it is and why, and what vaht asks of the client.
//!
//! A [`Window`] is owned only by [`Windows`](super::windows::Windows);
//! everything else refers to it by its [`WindowId`], so a closed window can
//! never be reached through a stale reference (01 §1). It holds no protocol
//! object: the compositor gives it each [`CommittedState`] and carries out
//! the [`RequestedState`] it asks for.
//!
//! What vaht asks and what the client has are kept apart on purpose: a
//! client takes a moment to apply a new size, and need not apply it
//! exactly. Areas follow the committed size, and a resize keeps the edges
//! opposite the dragged ones still as each new size arrives
//! ([`ResizeAnchor`]).

mod client;
mod edge_snap;
mod frame;
mod placement;
mod resize;

use std::fmt;

use smithay::utils::{Logical, Point, Rectangle, Size};
use vaht_ui::theme::Metrics;
use vaht_ui::{Theme, Typesetter};

pub(crate) use client::{CommittedState, Ending, PlacementKind, RequestedState};
pub(crate) use edge_snap::EdgeSnap;
pub(crate) use frame::{Frame, FrameGeometry, FramePart, TitlebarButton, TitlebarLayout};
pub(crate) use placement::{Area, Axes, Direction, OutputArea, Placement, SnapStep, TileCell};
pub(crate) use resize::{HorizontalEdge, ResizeAnchor, ResizeEdges, SizeLimits, VerticalEdge};

use super::workspaces::Workspace;
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

/// What dragging a window with the pointer does to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowDrag {
    /// Moves it.
    Move,
    /// Resizes it by dragging `edges`.
    Resize {
        /// The edges that follow the pointer.
        edges: ResizeEdges,
    },
}

/// What the user has done to a window besides placing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct WindowState {
    /// Hidden until restored.
    pub(crate) is_minimized: bool,
    /// Shown on every workspace.
    pub(crate) is_sticky: bool,
    /// Rolled up into its titlebar.
    pub(crate) is_shaded: bool,
}

/// A drag of the window in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragProgress {
    /// A move, which began with the content's corner at `origin`.
    Move {
        /// Where the content's corner was when the move began.
        origin: Point<i32, Logical>,
    },
    /// A resize, or one just finished whose last size is still to arrive.
    Resize(ResizeAnchor),
}

/// A managed window.
#[derive(Debug)]
pub(crate) struct Window {
    /// Its identity.
    id: WindowId,
    /// What its client last committed.
    committed: CommittedState,
    /// What vaht asks of its client.
    requested: RequestedState,
    /// Whether `requested` changed since the compositor last took it.
    is_request_pending: bool,
    /// Where the top-left corner of its content is, in global coordinates.
    location: Point<i32, Logical>,
    /// What vaht draws around it.
    frame: Frame,
    /// The drag in progress, if any.
    drag: Option<DragProgress>,
    /// Where it is and why.
    placement: Placement,
    /// Minimized, sticky, shaded.
    state: WindowState,
    /// The workspace it is on; ignored while it is sticky.
    workspace: Workspace,
    /// Whether it still waits for its first place, which needs its size.
    awaits_placement: bool,
    /// Counts the window while it exists.
    _tracked: Tracked,
}

impl Window {
    /// Creates the window `id` with `frame`, on `workspace`. It waits for
    /// its first place until its client commits a size.
    pub(crate) fn new(
        id: WindowId,
        frame: Frame,
        workspace: Workspace,
        ledger: &ResourceLedger,
    ) -> Self {
        Self {
            id,
            committed: CommittedState::default(),
            requested: RequestedState::default(),
            is_request_pending: false,
            location: Point::default(),
            frame,
            drag: None,
            placement: Placement::Floating,
            state: WindowState::default(),
            workspace,
            awaits_placement: true,
            _tracked: ledger.track(ResourceKind::Window),
        }
    }

    /// Returns the window's identity.
    pub(crate) const fn id(&self) -> WindowId {
        self.id
    }

    /// Returns the title its client set.
    pub(crate) fn title(&self) -> &str {
        &self.committed.title
    }

    /// Returns the application id its client set.
    pub(crate) fn app_id(&self) -> &str {
        &self.committed.app_id
    }

    /// Returns where the content's top-left corner is.
    pub(crate) const fn location(&self) -> Point<i32, Logical> {
        self.location
    }

    /// Returns where the window is and why.
    pub(crate) const fn placement(&self) -> &Placement {
        &self.placement
    }

    /// Returns whether it is minimized, sticky or shaded.
    pub(crate) const fn state(&self) -> WindowState {
        self.state
    }

    /// Returns the workspace it is on (all of them, while sticky).
    pub(crate) const fn workspace(&self) -> Workspace {
        self.workspace
    }

    /// Returns the frame.
    pub(crate) const fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Returns whether the window has had its first place.
    pub(crate) const fn is_placed(&self) -> bool {
        !self.awaits_placement
    }

    /// Returns whether the window still waits for its first place and has
    /// a size to choose it by.
    pub(crate) fn is_ready_for_placement(&self) -> bool {
        self.awaits_placement && !self.committed.geometry.size.is_empty()
    }

    /// Returns whether the window is shown while `current` is.
    pub(crate) fn is_shown_on(&self, current: Workspace) -> bool {
        !self.state.is_minimized && (self.state.is_sticky || self.workspace == current)
    }

    /// Returns whether vaht draws a frame around it: not while fullscreen.
    pub(crate) const fn has_frame(&self) -> bool {
        !self.placement.is_fullscreen()
    }

    /// Returns whether its content is shown: not while shaded.
    pub(crate) const fn shows_content(&self) -> bool {
        !self.state.is_shaded
    }

    /// Returns the area the window's content covers, in global coordinates.
    pub(crate) fn content_area(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.location, self.committed.geometry.size)
    }

    /// Returns the area the window covers on screen, frame included.
    pub(crate) fn outer_area(&self, metrics: Metrics) -> Area {
        if self.has_frame() {
            self.frame_geometry(metrics).outer
        } else {
            self.content_area()
        }
    }

    /// Returns where each part of the frame goes with the theme's `metrics`;
    /// a shaded window's frame closes up beneath its titlebar.
    pub(crate) fn frame_geometry(&self, metrics: Metrics) -> FrameGeometry {
        let mut content = self.content_area();
        if !self.shows_content() {
            content.size.h = 0;
        }
        FrameGeometry::around(content, metrics)
    }

    /// Returns where the client's surface is drawn: the content location,
    /// moved by the margin a client-side shadow adds around its content.
    pub(crate) fn surface_origin(&self) -> Point<i32, Logical> {
        self.location - self.committed.geometry.loc
    }

    /// Brings the window up to date with what its client committed: its
    /// title, its size, and its position while a resize keeps edges still.
    pub(crate) fn commit(&mut self, committed: CommittedState) {
        self.frame.set_title(&committed.title);
        self.committed = committed;
        if let Some(DragProgress::Resize(anchor)) = self.drag {
            let size = self.committed.geometry.size;
            self.location = anchor.edges.anchored_location(anchor.initial, size);
            if anchor.is_finishing {
                self.drag = None;
            }
        }
    }

    /// Shows the window as focused or not, and tells its client.
    pub(crate) fn set_focused(&mut self, is_focused: bool) {
        self.frame.set_focused(is_focused);
        self.request(|requested| requested.is_activated = is_focused);
    }

    /// Lays the titlebar's buttons out as `buttons`.
    pub(crate) fn set_titlebar_buttons(&mut self, buttons: &TitlebarLayout) {
        self.frame.set_buttons(buttons);
    }

    /// Paints the frame, sized by `metrics`, if anything changed.
    pub(crate) fn paint_frame(
        &mut self,
        metrics: Metrics,
        theme: &Theme,
        typesetter: &mut Typesetter,
    ) {
        let geometry = self.frame_geometry(metrics);
        self.frame.paint(&geometry, theme, typesetter);
    }

    /// Holds down the titlebar button at global `location`, if there is one
    /// there with frames sized by `metrics`; returns whether there was.
    pub(crate) fn press_titlebar_button(
        &mut self,
        location: Point<f64, Logical>,
        metrics: Metrics,
    ) -> bool {
        let point = self.titlebar_point(location, metrics);
        self.frame.press_button(point)
    }

    /// Lets go of the held titlebar button with the pointer at global
    /// `location`; returns the button if that clicks it.
    pub(crate) fn release_titlebar_button(
        &mut self,
        location: Point<f64, Logical>,
        metrics: Metrics,
    ) -> Option<TitlebarButton> {
        let point = self.titlebar_point(location, metrics);
        self.frame.release_button(point)
    }

    /// Puts the window where it first goes: its frame at `outer`. If that
    /// is not its size, the client is asked for the content size that fits.
    pub(crate) fn place_initially(&mut self, outer: Area, metrics: Metrics) {
        let content = FrameGeometry::content_within(outer, metrics);
        self.location = content.loc;
        if content.size != self.content_area().size {
            self.request_size(content.size);
        }
        self.awaits_placement = false;
    }

    /// Places the window so on `output`, its frame sized by `metrics`: moves
    /// it, and asks the client for the size and to show the placement. A
    /// drag in progress ends: the window is no longer the user's to drag.
    pub(crate) fn place(&mut self, placement: Placement, output: OutputArea, metrics: Metrics) {
        let current = self.outer_area(metrics);
        let outer = match placement {
            Placement::Floating => self.placement.restore_area(current),
            Placement::Maximized { .. }
            | Placement::Tiled { .. }
            | Placement::Fullscreen { .. } => placement.area(current, output.usable, output.whole),
        };
        let content = if placement.is_fullscreen() {
            outer
        } else {
            FrameGeometry::content_within(outer, metrics)
        };
        tracing::debug!(window = %self.id, ?placement, "placed");
        self.drag = None;
        self.location = content.loc;
        self.request(|requested| {
            requested.placement = placement.kind();
            requested.is_resizing = false;
        });
        self.placement = placement;
        self.request_size(content.size);
    }

    /// Minimizes the window, or restores it.
    pub(crate) const fn set_minimized(&mut self, is_minimized: bool) {
        self.state.is_minimized = is_minimized;
    }

    /// Shows the window on every workspace, or only its own.
    pub(crate) const fn set_sticky(&mut self, is_sticky: bool) {
        self.state.is_sticky = is_sticky;
    }

    /// Rolls the window up into its titlebar, or down again.
    pub(crate) const fn set_shaded(&mut self, is_shaded: bool) {
        self.state.is_shaded = is_shaded;
    }

    /// Puts the window on `workspace`.
    pub(crate) const fn move_to_workspace(&mut self, workspace: Workspace) {
        self.workspace = workspace;
    }

    /// Moves the window by `delta`, as a floating window.
    pub(crate) fn move_by(&mut self, delta: Point<i32, Logical>, metrics: Metrics) {
        self.float(None, metrics);
        self.location += delta;
    }

    /// Asks the client to make the content `size`, as a floating window.
    pub(crate) fn resize_to(&mut self, size: Size<i32, Logical>, metrics: Metrics) {
        self.float(None, metrics);
        self.request_size(size);
    }

    /// Starts `drag` with the pointer at `pointer`. A maximized or tiled
    /// window floats first: for a move, at its floating size under the
    /// pointer.
    pub(crate) fn begin_drag(
        &mut self,
        drag: WindowDrag,
        pointer: Point<f64, Logical>,
        metrics: Metrics,
    ) {
        match drag {
            WindowDrag::Move => {
                self.float(Some(pointer), metrics);
                self.drag = Some(DragProgress::Move {
                    origin: self.location,
                });
            }
            WindowDrag::Resize { edges } => {
                self.float(None, metrics);
                self.drag = Some(DragProgress::Resize(ResizeAnchor {
                    edges,
                    initial: self.content_area(),
                    is_finishing: false,
                }));
                self.request(|requested| requested.is_resizing = true);
            }
        }
    }

    /// Follows the drag in progress after the pointer moved `delta` since
    /// it began: moves the window, its edges snapping as `snap` says, or
    /// asks the client for the size the resize has reached.
    pub(crate) fn follow_drag(
        &mut self,
        delta: Point<i32, Logical>,
        snap: &EdgeSnap,
        metrics: Metrics,
    ) {
        match self.drag {
            Some(DragProgress::Move { origin }) => {
                let outer = self.outer_area(metrics);
                let moved = origin + delta;
                let moved_outer = Rectangle::new(outer.loc + (moved - self.location), outer.size);
                self.location = moved + (snap.snap(moved_outer) - moved_outer.loc);
            }
            Some(DragProgress::Resize(anchor)) if !anchor.is_finishing => {
                let limits = self.committed.size_limits;
                self.request_size(anchor.edges.resize(anchor.initial.size, delta, limits));
            }
            Some(DragProgress::Resize(_)) | None => {}
        }
    }

    /// Ends the drag in progress; a resize's last size from the client is
    /// still anchored when it arrives.
    pub(crate) fn end_drag(&mut self) {
        match &mut self.drag {
            Some(DragProgress::Move { .. }) => self.drag = None,
            Some(DragProgress::Resize(anchor)) => {
                anchor.is_finishing = true;
                self.request(|requested| requested.is_resizing = false);
            }
            None => {}
        }
    }

    /// Returns what vaht asks of the client, if that changed since it was
    /// last taken: the compositor carries it out.
    pub(crate) const fn take_request(&mut self) -> Option<RequestedState> {
        if self.is_request_pending {
            self.is_request_pending = false;
            Some(self.requested)
        } else {
            None
        }
    }

    /// Makes the window float where it is, so it can be moved or resized by
    /// hand. For a move with the pointer at `pointer`, a window that was
    /// maximized or tiled goes back to its floating size, the pointer at
    /// the same place across it.
    fn float(&mut self, pointer: Option<Point<f64, Logical>>, metrics: Metrics) {
        if self.placement == Placement::Floating {
            return;
        }
        let current = self.outer_area(metrics);
        let outer = match pointer {
            Some(pointer) => {
                let restore = self.placement.restore_area(current);
                let across = (pointer.x - f64::from(current.loc.x)) / f64::from(current.size.w);
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "an offset within a window's width fits in i32"
                )]
                let offset = (across * f64::from(restore.size.w)).round() as i32;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "pointer coordinates fit in i32"
                )]
                let left = pointer.x.round() as i32 - offset;
                Rectangle::new((left, current.loc.y).into(), restore.size)
            }
            None => current,
        };
        let content = FrameGeometry::content_within(outer, metrics);
        self.placement = Placement::Floating;
        self.location = content.loc;
        self.request(|requested| requested.placement = PlacementKind::Floating);
        self.request_size(content.size);
    }

    /// Changes what vaht asks of the client; it is sent if that changed.
    fn request(&mut self, change: impl FnOnce(&mut RequestedState)) {
        let before = self.requested;
        change(&mut self.requested);
        self.is_request_pending |= self.requested != before;
    }

    /// Asks the client for content of `size`. It is sent even when the
    /// size asked last was the same: the client may since have chosen
    /// another.
    const fn request_size(&mut self, size: Size<i32, Logical>) {
        self.requested.size = Some(size);
        self.is_request_pending = true;
    }

    /// Returns global `location` in the titlebar's own pixels.
    fn titlebar_point(&self, location: Point<f64, Logical>, metrics: Metrics) -> vaht_ui::Point {
        let inside = location.to_i32_floor() - self.frame_geometry(metrics).titlebar.loc;
        vaht_ui::Point::new(inside.x, inside.y)
    }
}

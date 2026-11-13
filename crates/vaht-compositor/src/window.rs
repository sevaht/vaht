//! One managed window: its identity, the client's side of it, and where it is.
//!
//! A [`Window`] is owned only by [`Windows`](crate::windows::Windows);
//! everything else refers to it by its [`WindowId`], so a closed window can
//! never be reached through a stale reference (01 §1).

mod edge_snap;
mod frame;
mod placement;
mod resize;
mod toplevel;

use std::fmt;

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Size};
use vaht_ui::theme::Metrics;

pub(crate) use edge_snap::EdgeSnap;
pub(crate) use frame::{Frame, FrameGeometry, FramePart, TitlebarButton, TitlebarLayout};
pub(crate) use placement::{Area, Axes, Direction, OutputArea, Placement, SnapStep, TileCell};
pub(crate) use resize::{HorizontalEdge, ResizeAnchor, ResizeEdges, VerticalEdge};
pub(crate) use toplevel::Toplevel;

use crate::resources::{ResourceKind, ResourceLedger, Tracked};
use crate::workspaces::Workspace;

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

/// A window about to be managed: its parts, before it has an id.
#[derive(Debug)]
pub(crate) struct NewWindow {
    /// The client's side.
    pub(crate) toplevel: Toplevel,
    /// The frame vaht draws around it.
    pub(crate) frame: Frame,
    /// The workspace it opens on.
    pub(crate) workspace: Workspace,
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
    /// Where the content's corner was when the move in progress began.
    move_origin: Option<Point<i32, Logical>>,
    /// The resize in progress, which positions the window as the client
    /// applies new sizes.
    resize: Option<ResizeAnchor>,
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
    /// Creates the window `new`, identified by `id`. It waits for its
    /// first place until the client has given it a size.
    pub(crate) fn new(id: WindowId, new: NewWindow, ledger: &ResourceLedger) -> Self {
        let NewWindow {
            toplevel,
            frame,
            workspace,
        } = new;
        Self {
            id,
            toplevel,
            location: Point::default(),
            frame,
            move_origin: None,
            resize: None,
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

    /// Returns the client's side of the window.
    pub(crate) const fn toplevel(&self) -> &Toplevel {
        &self.toplevel
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

    /// Returns whether the window has had its first place.
    pub(crate) const fn is_placed(&self) -> bool {
        !self.awaits_placement
    }

    /// Returns whether the window still waits for its first place and has
    /// a size to choose it by.
    pub(crate) fn is_ready_for_placement(&self) -> bool {
        self.awaits_placement && !self.toplevel.geometry().size.is_empty()
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
        Rectangle::new(self.location, self.toplevel.geometry().size)
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

    /// Returns the frame.
    pub(crate) const fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Returns the frame, to change it.
    pub(crate) const fn frame_mut(&mut self) -> &mut Frame {
        &mut self.frame
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

    /// Puts the window where it first goes: its frame's corner at `corner`.
    pub(crate) fn place_initially(&mut self, corner: Point<i32, Logical>, metrics: Metrics) {
        let outer = Rectangle::new(corner, self.outer_area(metrics).size);
        self.location = FrameGeometry::content_within(outer, metrics).loc;
        self.awaits_placement = false;
    }

    /// Places the window so on `output`, its frame sized by `metrics`: moves
    /// it, asks the client for the size, and tells the client its state.
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
        self.placement = placement;
        self.resize = None;
        self.location = content.loc;
        self.toplevel
            .configure_placement(&self.placement, content.size);
    }

    /// Makes the window float where it is, so it can be moved or resized by
    /// hand. For a move with the pointer at `pointer`, a window that was
    /// maximized or tiled goes back to its floating size, the pointer at
    /// the same place across it.
    pub(crate) fn float(&mut self, pointer: Option<Point<f64, Logical>>, metrics: Metrics) {
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
        self.toplevel
            .configure_placement(&self.placement, content.size);
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
        self.toplevel.request_size(size);
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
                self.move_origin = Some(self.location);
            }
            WindowDrag::Resize { edges } => {
                self.float(None, metrics);
                self.resize = Some(ResizeAnchor {
                    edges,
                    initial: self.content_area(),
                    is_finishing: false,
                });
                self.toplevel.set_resizing(true);
            }
        }
    }

    /// Applies `drag` after the pointer moved `delta` since it began: moves
    /// the window, its edges snapping as `snap` says, or asks the client for
    /// the size the resize has reached.
    pub(crate) fn follow_drag(
        &mut self,
        drag: WindowDrag,
        delta: Point<i32, Logical>,
        snap: &EdgeSnap,
        metrics: Metrics,
    ) {
        match drag {
            WindowDrag::Move => {
                if let Some(origin) = self.move_origin {
                    let outer = self.outer_area(metrics);
                    let moved = origin + delta;
                    let moved_outer =
                        Rectangle::new(outer.loc + (moved - self.location), outer.size);
                    self.location = moved + (snap.snap(moved_outer) - moved_outer.loc);
                }
            }
            WindowDrag::Resize { .. } => {
                if let Some(anchor) = self.resize {
                    let limits = self.toplevel.size_limits();
                    let size: Size<i32, Logical> =
                        anchor.edges.resize(anchor.initial.size, delta, limits);
                    self.toplevel.request_size(size);
                }
            }
        }
    }

    /// Ends `drag`; a resize's last size from the client is still anchored.
    pub(crate) fn end_drag(&mut self, drag: WindowDrag) {
        match (drag, &mut self.resize) {
            (WindowDrag::Move, _) => self.move_origin = None,
            (WindowDrag::Resize { .. }, Some(anchor)) => {
                anchor.is_finishing = true;
                self.toplevel.set_resizing(false);
            }
            (WindowDrag::Resize { .. }, None) => {}
        }
    }

    /// Brings vaht's view up to date with what the client last committed:
    /// its title, and its position while a resize keeps an edge still.
    pub(crate) fn sync_with_client(&mut self) {
        let title = self.toplevel.title().unwrap_or_default();
        self.frame.set_title(&title);
        if let Some(anchor) = self.resize {
            let size = self.toplevel.geometry().size;
            self.location = anchor.edges.anchored_location(anchor.initial, size);
            if anchor.is_finishing {
                self.resize = None;
            }
        }
    }

    /// Returns global `location` in the titlebar's own pixels.
    fn titlebar_point(&self, location: Point<f64, Logical>, metrics: Metrics) -> vaht_ui::Point {
        let inside = location.to_i32_floor() - self.frame_geometry(metrics).titlebar.loc;
        vaht_ui::Point::new(inside.x, inside.y)
    }
}

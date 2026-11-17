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

mod alpha;
mod client;
mod edge_snap;
mod exemptions;
mod frame;
mod placement;
mod resize;

use std::fmt;

use smithay::utils::{Logical, Point, Rectangle, Size};
use vaht_ui::theme::Metrics;
use vaht_ui::{Theme, Typesetter};

pub(crate) use alpha::{AlphaChange, WindowAlpha};
pub(crate) use client::{ClientIcon, CommittedState, Ending, PlacementKind, RequestedState};
pub(crate) use edge_snap::EdgeSnap;
pub(crate) use exemptions::{Exemption, Exemptions};
pub(crate) use frame::{
    Frame, FrameGeometry, FramePart, FrameShape, GripSide, TitlebarButton, TitlebarLayout,
};
pub(crate) use placement::{Area, Axes, Direction, OutputArea, Placement, SnapStep, TileCell};
pub(crate) use resize::{HorizontalEdge, ResizeAnchor, ResizeEdges, SizeLimits, VerticalEdge};

use super::workspaces::Workspace;
use crate::config::FocusStealing;
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

/// Whether vaht frames a window, as `toggle-frame` leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FrameChoice {
    /// Framed, unless fullscreen or drawing its own.
    #[default]
    Framed,
    /// Framed even when it draws its own (a rule's `frame = "full"`, as
    /// Fluxbox's `[Deco]` forces one), unless fullscreen.
    Forced,
    /// Never framed.
    Frameless,
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
    /// A resize shown as an outline: the client is asked for nothing until
    /// it ends, at `size`.
    Outline {
        /// The edges dragged, and the content area it began with.
        anchor: ResizeAnchor,
        /// The content size reached.
        size: Size<i32, Logical>,
    },
}

/// How a resize by hand is shown while it lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResizeStyle {
    /// The window itself takes each size the drag reaches.
    Live,
    /// An outline shows the size, which the window takes when the drag
    /// ends (Fluxbox's `opaqueResize: false`).
    Outline,
}

/// A managed window.
#[derive(Debug)]
pub(crate) struct Window {
    /// Its identity.
    id: WindowId,
    /// Whether vaht frames it.
    frame_choice: FrameChoice,
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
    /// Whether only the wallpaper shows through it where it is
    /// translucent, as a rule says.
    is_pseudo_transparent: bool,
    /// Its opacity, as `set-alpha` set it; the settings' when none.
    alpha: Option<WindowAlpha>,
    /// Its title, as `set-title` set it; its client's when none.
    title: Option<String>,
    /// Whether it may come forward when its program asks, as a rule says.
    focus_stealing: FocusStealing,
    /// What window it is a dialog of, if it is one.
    dialog: Option<Dialog>,
    /// What rules exempt it from.
    exemptions: Exemptions,
    /// Counts the window while it exists.
    _tracked: Tracked,
}

/// What makes a window a dialog: the window it belongs to, and whether it
/// keeps the focus from that one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Dialog {
    /// The window it is a dialog of (its transient parent).
    pub(crate) parent: WindowId,
    /// Whether it keeps the focus from its parent while shown (a modal
    /// dialog).
    pub(crate) is_modal: bool,
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
            frame_choice: FrameChoice::Framed,
            workspace,
            awaits_placement: true,
            is_pseudo_transparent: false,
            alpha: None,
            title: None,
            focus_stealing: FocusStealing::Default,
            dialog: None,
            exemptions: Exemptions::default(),
            _tracked: ledger.track(ResourceKind::Window),
        }
    }

    /// Returns the window's identity.
    pub(crate) const fn id(&self) -> WindowId {
        self.id
    }

    /// Returns its title: the one `set-title` gave it, else its client's.
    pub(crate) fn title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.committed.title)
    }

    /// Gives the window `title` in place of its client's, or with none its
    /// client's again.
    pub(crate) fn set_title(&mut self, title: Option<String>) {
        self.title = title;
        let shown = self.title().to_owned();
        self.frame.set_title(&shown);
    }

    /// Returns the window's icon at `size` pixels, looked up in `store`:
    /// the one its client named or drew, else its application's.
    pub(crate) fn icon_in(
        &self,
        store: &mut vaht_ui::IconStore,
        size: u32,
    ) -> Option<vaht_ui::Icon> {
        match self.icon() {
            Some(ClientIcon::Named(name)) => store.named(name, size),
            Some(ClientIcon::Pixels(icon)) => Some(icon.clone()),
            None => None,
        }
        .or_else(|| store.app_icon(self.app_id(), size))
    }

    /// Returns the application id its client set.
    pub(crate) fn app_id(&self) -> &str {
        &self.committed.app_id
    }

    /// Returns the icon its client chose, if it chose one.
    pub(crate) const fn icon(&self) -> Option<&ClientIcon> {
        self.committed.icon.as_ref()
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

    /// Returns whether the window may come forward when its program asks.
    pub(crate) const fn focus_stealing(&self) -> FocusStealing {
        self.focus_stealing
    }

    /// Says whether the window may come forward when its program asks.
    pub(crate) const fn set_focus_stealing(&mut self, focus_stealing: FocusStealing) {
        self.focus_stealing = focus_stealing;
    }

    /// Returns how opaque the window is drawn, 0.0 to 1.0: as `set-alpha`
    /// set it, else as `defaults` say, for whether it has focus.
    pub(crate) fn opacity(&self, defaults: WindowAlpha) -> f32 {
        let is_focused = self.frame.state() == vaht_ui::WidgetState::Focused;
        self.alpha.unwrap_or(defaults).opacity(is_focused)
    }

    /// Changes the window's opacity as `change` says, from what it is with
    /// `defaults`.
    pub(crate) fn change_alpha(&mut self, change: AlphaChange, defaults: WindowAlpha) {
        self.alpha = change.applied(self.alpha.unwrap_or(defaults));
    }

    /// Returns whether only the wallpaper shows through the window where it
    /// is translucent, not the windows below it.
    pub(crate) const fn is_pseudo_transparent(&self) -> bool {
        self.is_pseudo_transparent
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

    /// Returns whether vaht draws a frame around it: not while fullscreen,
    /// nor around a window that draws its own and asks for none.
    pub(crate) const fn has_frame(&self) -> bool {
        self.is_framed_when(&self.placement)
    }

    /// Returns whether it has a frame when placed as `placement`.
    const fn is_framed_when(&self, placement: &Placement) -> bool {
        !placement.is_fullscreen()
            && match self.frame_choice {
                FrameChoice::Forced => true,
                FrameChoice::Framed => !self.committed.is_self_decorated,
                FrameChoice::Frameless => false,
            }
    }

    /// Returns where its content goes when its frame (if it has one when
    /// placed as `placement`) covers `outer`.
    fn content_within(&self, outer: Area, placement: &Placement, metrics: Metrics) -> Area {
        if self.is_framed_when(placement) {
            FrameGeometry::content_within(outer, metrics)
        } else {
            outer
        }
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
        let shape = if self.shows_content() {
            FrameShape::Open
        } else {
            FrameShape::Shaded
        };
        FrameGeometry::around(self.content_area(), metrics, shape)
    }

    /// Returns where the client's surface is drawn: the content location,
    /// moved by the margin a client-side shadow adds around its content.
    pub(crate) fn surface_origin(&self) -> Point<i32, Logical> {
        self.location - self.committed.geometry.loc
    }

    /// Returns whether a titlebar button is at global `location`, with
    /// frames sized by `metrics`.
    pub(crate) fn has_titlebar_button_at(
        &self,
        location: Point<f64, Logical>,
        metrics: Metrics,
    ) -> bool {
        self.frame
            .has_button_at(self.titlebar_point(location, metrics))
    }

    /// Brings the window up to date with what its client committed: its
    /// title, its size, and its position while a resize keeps edges still.
    pub(crate) fn commit(&mut self, committed: CommittedState) {
        self.committed = committed;
        let shown = self.title().to_owned();
        self.frame.set_title(&shown);
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

    /// Marks the frame for painting again, as when the theme changed.
    pub(crate) fn invalidate_frame(&mut self) {
        self.frame.invalidate();
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

    /// Shows its icon at `size` pixels, looked up in `store`, on its
    /// titlebar; or none (`store` is none, or no icon is found).
    pub(crate) fn show_frame_icon(&mut self, store: Option<&mut vaht_ui::IconStore>, size: u32) {
        let icon = store.and_then(|store| self.icon_in(store, size));
        self.frame.set_icon(icon);
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
        let content = self.content_within(outer, &self.placement, metrics);
        self.location = content.loc;
        if content.size != self.content_area().size {
            self.request_size(content.size);
        }
        self.awaits_placement = false;
        tracing::debug!(window = %self.id, area = ?outer, "placed first");
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
        let content = self.content_within(outer, &placement, metrics);
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

    /// Lets only the wallpaper show through the window where it is
    /// translucent, or the windows below it too.
    pub(crate) const fn set_pseudo_transparent(&mut self, is_pseudo_transparent: bool) {
        self.is_pseudo_transparent = is_pseudo_transparent;
    }

    /// Returns the window it is a dialog of, if any.
    pub(crate) fn parent(&self) -> Option<WindowId> {
        self.dialog.map(|dialog| dialog.parent)
    }

    /// Returns whether it is a modal dialog of its parent.
    pub(crate) fn is_modal(&self) -> bool {
        self.dialog.is_some_and(|dialog| dialog.is_modal)
    }

    /// Returns what window it is a dialog of, if it is one.
    pub(crate) const fn dialog(&self) -> Option<Dialog> {
        self.dialog
    }

    /// Makes the window `dialog`, or with `None` a window of its own.
    pub(crate) const fn set_dialog(&mut self, dialog: Option<Dialog>) {
        self.dialog = dialog;
    }

    /// Returns whether a rule exempts it from `exemption`.
    pub(crate) const fn is_exempt(&self, exemption: Exemption) -> bool {
        self.exemptions.has(exemption)
    }

    /// Exempts it from `exemption` if `is_exempt`, or not.
    pub(crate) const fn set_exempt(&mut self, exemption: Exemption, is_exempt: bool) {
        self.exemptions = self.exemptions.with(exemption, is_exempt);
    }

    /// Gives it the opacities `alpha`, as a rule says.
    pub(crate) const fn set_alpha(&mut self, alpha: WindowAlpha) {
        self.alpha = Some(alpha);
    }

    /// Frames the window, even if it draws its own, or with `false` not,
    /// whatever it was before: as a rule says.
    pub(crate) fn set_framed(&mut self, is_framed: bool) {
        let choice = if is_framed {
            FrameChoice::Forced
        } else {
            FrameChoice::Frameless
        };
        if self.frame_choice != choice {
            self.frame_choice = choice;
            self.invalidate_frame();
        }
    }

    /// Returns the content sizes its resizing by hand is held to: its
    /// client's, unless a rule exempts it.
    fn size_limits(&self) -> SizeLimits {
        if self.is_exempt(Exemption::SizeHints) {
            SizeLimits::default()
        } else {
            self.committed.size_limits
        }
    }

    /// Rolls the window up into its titlebar, or down again.
    pub(crate) const fn set_shaded(&mut self, is_shaded: bool) {
        self.state.is_shaded = is_shaded;
    }

    /// Takes the window's frame away if it shows one, or gives it one,
    /// even if it draws its own, if it shows none (`toggle-frame`).
    pub(crate) fn toggle_frame(&mut self) {
        self.frame_choice = if self.is_framed_when(&Placement::Floating) {
            FrameChoice::Frameless
        } else {
            FrameChoice::Forced
        };
        self.invalidate_frame();
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
        style: ResizeStyle,
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
                let anchor = ResizeAnchor {
                    edges,
                    initial: self.content_area(),
                    is_finishing: false,
                };
                match style {
                    ResizeStyle::Live => {
                        self.drag = Some(DragProgress::Resize(anchor));
                        self.request(|requested| requested.is_resizing = true);
                    }
                    ResizeStyle::Outline => {
                        self.drag = Some(DragProgress::Outline {
                            anchor,
                            size: anchor.initial.size,
                        });
                    }
                }
            }
        }
    }

    /// Returns where the frame would be if an outline resize ended now;
    /// none unless one is in progress.
    pub(crate) fn outline(&self, metrics: Metrics) -> Option<Area> {
        let Some(DragProgress::Outline { anchor, size }) = self.drag else {
            return None;
        };
        let location = anchor.edges.anchored_location(anchor.initial, size);
        let content = Rectangle::new(location, size);
        Some(if self.has_frame() {
            FrameGeometry::around(content, metrics, FrameShape::Open).outer
        } else {
            content
        })
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
                let limits = self.size_limits();
                self.request_size(anchor.edges.resize(anchor.initial.size, delta, limits));
            }
            Some(DragProgress::Outline { anchor, .. }) => {
                let limits = self.size_limits();
                let size = anchor.edges.resize(anchor.initial.size, delta, limits);
                self.drag = Some(DragProgress::Outline { anchor, size });
            }
            Some(DragProgress::Resize(_)) | None => {}
        }
    }

    /// Returns whether the window is being moved by hand.
    pub(crate) const fn is_moving(&self) -> bool {
        matches!(self.drag, Some(DragProgress::Move { .. }))
    }

    /// Ends a move, putting the window back where it began.
    pub(crate) fn cancel_move(&mut self) {
        if let Some(DragProgress::Move { origin }) = self.drag {
            self.location = origin;
            self.drag = None;
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
            Some(DragProgress::Outline { anchor, size }) => {
                let (anchor, size) = (*anchor, *size);
                if size == anchor.initial.size {
                    self.drag = None;
                } else {
                    // The size arrives as a live resize's last one does.
                    self.drag = Some(DragProgress::Resize(ResizeAnchor {
                        is_finishing: true,
                        ..anchor
                    }));
                    self.request_size(size);
                }
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
        let content = self.content_within(outer, &Placement::Floating, metrics);
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

//! The window manager: windows, workspaces and focus, and every action on
//! them, with no Wayland in it (DESIGN.md, "Where behavior lives").
//!
//! [`WindowManager`] decides everything about windows. The compositor feeds
//! it what clients commit ([`CommittedState`]) and what they ask
//! ([`ClientRequest`]), runs its actions ([`WindowManagerAction`]), and
//! carries out what it decides:
//! - each window's [`RequestedState`] and [`Ending`], taken together as
//!   [`ClientUpdates`];
//! - keyboard focus, which it reads from [`WindowManager::focused`];
//! - a new frame, when [`WindowManager::take_redraw`] says one is needed.
//!
//! It holds no protocol object and never calls Smithay's seat, so every
//! rule here is tested on its own, by unit and property tests below.

mod arrange;
mod direction;
mod family;
mod focus;
mod output_areas;
mod placement_strategy;
mod snap_zones;
mod window;
mod windows;
mod workspaces;

use smithay::utils::{Logical, Point, Rectangle};
use vaht_ui::style::Metrics;
use vaht_ui::{IconStore, Style, Typesetter, WidgetIcon};

pub(crate) use self::arrange::ArrangeStyle;
use self::focus::Focus;
pub(crate) use self::focus::{CycleDirection, FocusPlace};
pub(crate) use self::output_areas::{OutputAreas, OutputTarget};
pub(crate) use self::placement_strategy::{ColumnDirection, PlacementStrategy, Room, RowDirection};
use self::snap_zones::{LayoutPicker, SnapTarget};
pub(crate) use self::window::AlphaChange;
pub(crate) use self::window::Exemption;
pub use self::window::WindowId;
pub(crate) use self::window::{
    Area, Axes, ClientIcon, CommittedState, Direction, EdgeSnap, Ending, Frame, FrameGeometry,
    FramePart, GripSide, HorizontalEdge, OutputArea, Placement, PlacementKind, RequestedState,
    ResizeEdges, ResizeStyle, SizeLimits, SnapStep, TileCell, TitlebarButton, TitlebarLayout,
    VerticalEdge, Window, WindowAlpha, WindowDrag,
};
pub(crate) use self::windows::StackLayer;
use self::windows::Windows;
use self::workspaces::Workspaces;
pub(crate) use self::workspaces::{Workspace, WorkspaceTarget};
use crate::config::{
    Config, CycleOrder, FocusStealing, FrameSettings, RuleFrame, RuleOutput, RuleSettings,
    Settings, SnapSettings, WindowCycle, WindowManagerAction,
};
use crate::resources::ResourceLedger;

/// Where the window being moved would snap.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DragSnap {
    /// The window.
    window: WindowId,
    /// Where it would go, if anywhere.
    target: Option<SnapTarget>,
    /// The usable area of the output the pointer is on, which the target
    /// is a part of.
    usable: Area,
    /// The layout picker, if it shows, and the layout and cell under the
    /// pointer.
    picker: Option<(LayoutPicker, Option<(usize, usize)>)>,
}

/// The layout picker as it shows.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ShownPicker<'manager> {
    /// Where it and its thumbnails are.
    pub(crate) picker: &'manager LayoutPicker,
    /// The layouts, one per thumbnail.
    pub(crate) layouts: &'manager [Vec<TileCell>],
    /// The layout and cell under the pointer.
    pub(crate) hovered: Option<(usize, usize)>,
}

/// What a client asks for its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ClientRequest {
    /// To fill the usable area.
    Maximize,
    /// To stop filling it.
    Unmaximize,
    /// To cover the whole output.
    Fullscreen,
    /// To stop covering it.
    Unfullscreen,
    /// To be minimized.
    Minimize,
}

/// What the compositor must tell clients: each changed [`RequestedState`],
/// and the windows asked to end.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ClientUpdates {
    /// The windows whose requested state changed, and that state.
    pub(crate) states: Vec<(WindowId, RequestedState)>,
    /// The windows asked to end, and how.
    pub(crate) endings: Vec<(WindowId, Ending)>,
}

/// The windows, the workspaces and focus, and every rule about them.
#[derive(Debug)]
pub(crate) struct WindowManager {
    /// Every window, and their stacking order.
    windows: Windows,
    /// The workspaces, and which one is shown.
    workspaces: Workspaces,
    /// Which window has keyboard focus, and which had it before.
    focus: Focus,
    /// The window a snap step down just minimized: a step up restores it
    /// while the modifiers that went down for it are held.
    snap_minimized: Option<WindowId>,
    /// The window whose titlebar button the pointer holds down, if any.
    held_titlebar: Option<WindowId>,
    /// The window whose titlebar button the pointer is on, if any.
    hovered_titlebar: Option<WindowId>,
    /// Where windows can be placed on each output.
    outputs: OutputAreas,
    /// The style's frame sizes.
    style_metrics: Metrics,
    /// What frames show, from the settings.
    frame: FrameSettings,
    /// The frame sizes windows are laid out by: the style's, as `frame`
    /// takes them.
    metrics: Metrics,
    /// How a resize by hand is shown, from the settings.
    resize_style: ResizeStyle,
    /// How opaque windows are unless `set-alpha` says, from the settings.
    default_alpha: WindowAlpha,
    /// Snapping dragged windows to the output's edges, from the settings.
    snapping: SnapSettings,
    /// Where the window being moved would snap, while one is.
    drag_snap: Option<DragSnap>,
    /// Windows asked to end, until the compositor takes them.
    endings: Vec<(WindowId, Ending)>,
    /// Whether anything shown changed since the compositor last asked.
    is_redraw_needed: bool,
    /// The windows marked (`mark-window`), each with its key.
    marks: Vec<(String, WindowId)>,
}

impl WindowManager {
    /// Creates a window manager with no windows, its workspaces and frames
    /// as `settings` say, placing windows on `output` with frames sized by
    /// the style's `metrics`.
    pub(crate) fn new(settings: &Settings, outputs: OutputAreas, metrics: Metrics) -> Self {
        let frame = settings.windows.frame.clone();
        Self {
            windows: Windows::default(),
            workspaces: Workspaces::named(settings.workspaces.names()),
            focus: Focus::default(),
            snap_minimized: None,
            held_titlebar: None,
            hovered_titlebar: None,
            outputs,
            style_metrics: metrics,
            metrics: frame.frame_metrics(metrics),
            frame,
            resize_style: Self::resize_style(settings),
            default_alpha: settings.windows.alpha(),
            snapping: settings.windows.snapping.clone(),
            drag_snap: None,
            endings: Vec::new(),
            is_redraw_needed: false,
            marks: Vec::new(),
        }
    }

    /// Returns how `settings` say a resize by hand is shown.
    const fn resize_style(settings: &Settings) -> ResizeStyle {
        if settings.windows.opaque_resize {
            ResizeStyle::Live
        } else {
            ResizeStyle::Outline
        }
    }

    /// Returns whether `window` is in the above-dock layer: over the
    /// taskbar.
    pub(crate) fn is_above_dock(&self, window: WindowId) -> bool {
        self.windows.layer_of(window) == Some(StackLayer::AboveDock)
    }

    /// Returns the layer `window` is in.
    pub(crate) fn layer_of(&self, window: WindowId) -> Option<StackLayer> {
        self.windows.layer_of(window)
    }

    /// Gives `window` `title` in place of its client's, or its client's
    /// again with none.
    fn set_title(&mut self, window: Option<WindowId>, title: Option<String>) {
        if let Some(target) = window.and_then(|window| self.window_mut(window)) {
            target.set_title(title);
            self.is_redraw_needed = true;
        }
    }

    /// Moves `window` to the top of `layer`.
    fn set_layer(&mut self, window: Option<WindowId>, layer: StackLayer) {
        if let Some(window) = window
            && self.windows.set_layer(window, layer)
        {
            self.is_redraw_needed = true;
        }
    }

    /// Changes `window`'s opacity as `change` says.
    fn change_alpha(&mut self, window: Option<WindowId>, change: AlphaChange) {
        let defaults = self.default_alpha;
        if let Some(target) = window.and_then(|window| self.window_mut(window)) {
            target.change_alpha(change, defaults);
            self.is_redraw_needed = true;
        }
    }

    /// Returns how opaque `window` is drawn, 0.0 to 1.0.
    pub(crate) fn opacity(&self, window: &Window) -> f32 {
        window.opacity(self.default_alpha)
    }

    /// Returns where an outline resize in progress would put its window's
    /// frame; none without one.
    pub(crate) fn outline(&self) -> Option<Area> {
        let metrics = self.metrics;
        self.windows_top_down()
            .find_map(|window| window.outline(metrics))
    }

    /// Returns the window with keyboard focus.
    pub(crate) const fn focused(&self) -> Option<WindowId> {
        self.focus.focused()
    }

    /// Returns the workspace shown.
    pub(crate) const fn current_workspace(&self) -> Workspace {
        self.workspaces.current()
    }

    /// Returns every workspace, first to last, with its name.
    pub(crate) fn workspaces(&self) -> impl Iterator<Item = (Workspace, &str)> {
        self.workspaces
            .all()
            .map(|workspace| (workspace, self.workspaces.name(workspace)))
    }

    /// Adds a workspace at the end, named `name`, or as workspaces are by
    /// default if `name` is empty.
    pub(crate) fn add_workspace_named(&mut self, name: &str) {
        let added = if name.is_empty() {
            self.workspaces.add()
        } else {
            self.workspaces.add_named(name)
        };
        tracing::debug!(workspace = %added, "workspace added");
        self.is_redraw_needed = true;
    }

    /// Returns the name of the workspace shown.
    pub(crate) fn current_workspace_name(&self) -> &str {
        self.workspaces.name(self.workspaces.current())
    }

    /// Returns the window with `id`, if it still exists.
    pub(crate) fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(id)
    }

    /// Returns how many windows there are.
    pub(crate) fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Returns the frame sizes windows are laid out by: the style's, less
    /// what the settings leave out of frames.
    pub(crate) const fn metrics(&self) -> Metrics {
        self.metrics
    }

    /// Returns every window, from top to bottom.
    pub(crate) fn windows_top_down(&self) -> impl Iterator<Item = &Window> + '_ {
        self.windows.top_down()
    }

    /// Returns the windows shown, from top to bottom: the order drawing and
    /// hit-testing use.
    pub(crate) fn shown_top_down(&self) -> impl Iterator<Item = &Window> + '_ {
        self.windows.shown_top_down(self.workspaces.current())
    }

    /// Returns whether a titlebar button of `window` is at `location`.
    pub(crate) fn has_titlebar_button_at(
        &self,
        window: WindowId,
        location: Point<f64, Logical>,
    ) -> bool {
        self.windows
            .get(window)
            .is_some_and(|target| target.has_titlebar_button_at(location, self.metrics))
    }

    /// Starts managing a new window, on top of the current workspace and
    /// focused if `settings` say new windows take focus. It is placed once
    /// its client commits a size.
    pub(crate) fn add_window(&mut self, settings: &Settings, ledger: &ResourceLedger) -> WindowId {
        self.add_window_on(
            settings,
            ledger,
            self.workspaces.current(),
            settings.focus.focus_new_windows,
        )
    }

    /// Starts managing a new window on `workspace`, focused if `is_focused`:
    /// for one that takes the place of another of its program's, which
    /// keeps where the other was and whether it had the focus.
    pub(crate) fn add_window_on(
        &mut self,
        settings: &Settings,
        ledger: &ResourceLedger,
        workspace: Workspace,
        is_focused: bool,
    ) -> WindowId {
        let frame = Frame::new(settings.windows.frame.titlebar_layout(), ledger);
        let id = self.windows.insert(frame, workspace, ledger);
        tracing::debug!(window = %id, "window opened");
        if is_focused {
            self.focus(Some(id));
        }
        id
    }

    /// Stops managing the window `id`, which is gone.
    pub(crate) fn remove_window(&mut self, id: WindowId) {
        let Some(window) = self.windows.remove(id) else {
            return;
        };
        tracing::debug!(window = %id, app_id = window.app_id(), "window closed");
        let was_focused = self.focused() == Some(id);
        self.focus.forget(id);
        if self.snap_minimized == Some(id) {
            self.snap_minimized = None;
        }
        if self.held_titlebar == Some(id) {
            self.held_titlebar = None;
        }
        if self.hovered_titlebar == Some(id) {
            self.hovered_titlebar = None;
        }
        self.endings.retain(|(ending, _)| *ending != id);
        self.marks.retain(|(_, marked)| *marked != id);
        self.orphan_dialogs_of(id);
        if was_focused {
            self.focus_most_recent();
        }
        self.is_redraw_needed = true;
    }

    /// Brings the window `id` up to date with what its client committed,
    /// and gives it its first place once it has a size: by `config`'s
    /// rules and placement settings, with the pointer at `pointer`.
    pub(crate) fn commit(
        &mut self,
        id: WindowId,
        committed: CommittedState,
        config: &Config,
        pointer: Point<i32, Logical>,
    ) {
        let Some(window) = self.window_mut(id) else {
            return;
        };
        window.commit(committed);
        if window.is_ready_for_placement() {
            self.place_new_window(id, config, pointer);
        }
    }

    /// Gives keyboard focus to `window`, or with `None` to nothing. A
    /// window out of sight cannot have it, so focus stays where it is.
    pub(crate) fn focus(&mut self, window: Option<WindowId>) {
        // A modal dialog keeps the focus from the window it belongs to.
        let window = window.map(|id| self.focus_for(id));
        let current = self.workspaces.current();
        let is_focusable = window.is_none_or(|id| {
            self.windows
                .get(id)
                .is_some_and(|window| window.is_shown_on(current))
        });
        if !is_focusable {
            return;
        }
        let previous = self.focused();
        if previous != window {
            if let Some(unfocused) = previous.and_then(|id| self.window_mut(id)) {
                unfocused.set_focused(false);
            }
            if let Some(focused) = window.and_then(|id| self.window_mut(id)) {
                focused.set_focused(true);
                // The application, not the title: titles name documents,
                // pages and people, and the log keeps them on disk.
                tracing::debug!(window = %focused.id(), app_id = focused.app_id(), "focused");
            }
        }
        self.focus.set(window);
    }

    /// Brings `window` into view and gives it focus: restores it, with its
    /// dialogs, if it is minimized, shows its workspace, and raises it.
    pub(crate) fn activate(&mut self, window: WindowId) {
        for member in self.family_bottom_up(window) {
            if let Some(member) = self.window_mut(member) {
                member.set_minimized(false);
            }
        }
        let Some(target) = self.windows.get(window) else {
            return;
        };
        let (workspace, is_sticky) = (target.workspace(), target.state().is_sticky);
        if !is_sticky {
            self.show(workspace);
        }
        self.raise(window);
        self.focus(Some(window));
    }

    /// Minimizes `window` if it has focus and is shown, as clicking the
    /// focused window's taskbar entry does; otherwise brings it into view
    /// and gives it focus.
    pub(crate) fn activate_or_minimize(&mut self, window: WindowId) {
        let current = self.workspaces.current();
        let is_in_front = self.focused() == Some(window)
            && self
                .windows
                .get(window)
                .is_some_and(|target| target.is_shown_on(current));
        if is_in_front {
            self.minimize(Some(window));
        } else {
            self.activate(window);
        }
    }

    /// Returns where `window` stands in the order windows had focus in.
    pub(crate) fn focus_place_of(&self, window: WindowId) -> FocusPlace {
        self.focus.place_of(window)
    }

    /// Puts `window` where `place` was in the order windows had focus in.
    pub(crate) fn give_focus_place(&mut self, window: WindowId, place: FocusPlace) {
        self.focus.give_place(window, place);
    }

    /// Returns the window directly above `window`, if one is.
    pub(crate) fn window_above(&self, window: WindowId) -> Option<WindowId> {
        self.windows.above(window)
    }

    /// Moves `window` to directly below `other`, if both are in the same
    /// layer.
    pub(crate) fn place_below(&mut self, window: WindowId, other: WindowId) {
        self.is_redraw_needed |= self.windows.place_below(window, other);
    }

    /// Moves `window` above all others, its dialogs above it.
    pub(crate) fn raise(&mut self, window: WindowId) {
        for member in self.family_bottom_up(window) {
            self.is_redraw_needed |= self.windows.raise(member);
        }
    }

    /// Moves `window` below all others, its dialogs still above it.
    pub(crate) fn lower(&mut self, window: WindowId) {
        for member in self.family_bottom_up(window).into_iter().rev() {
            self.is_redraw_needed |= self.windows.lower(member);
        }
    }

    /// Runs `action` on `window`: the one clicked for a mouse binding, the
    /// focused one for a key binding.
    pub(crate) fn run(&mut self, action: &WindowManagerAction, window: Option<WindowId>) {
        match action {
            WindowManagerAction::Close => self.end(window, Ending::Close),
            WindowManagerAction::Kill => self.end(window, Ending::Kill),
            WindowManagerAction::Raise => {
                if let Some(window) = window {
                    self.raise(window);
                }
            }
            WindowManagerAction::Lower => {
                if let Some(window) = window {
                    self.lower(window);
                }
            }
            WindowManagerAction::Focus => {
                if window.is_some() {
                    self.focus(window);
                }
            }
            WindowManagerAction::Maximize(axes) => self
                .change_placement(window, |placement, current| {
                    placement.with_maximized_toggled(*axes, current)
                }),
            WindowManagerAction::Fullscreen => {
                self.change_placement(window, Placement::with_fullscreen_toggled);
            }
            WindowManagerAction::SnapStep(direction) => self.snap_step(window, *direction),
            WindowManagerAction::Tile(cell) => {
                self.change_placement(window, |placement, current| Placement::Tiled {
                    cell: *cell,
                    restore: placement.restore_area(current),
                });
            }
            WindowManagerAction::Minimize => self.minimize(window),
            WindowManagerAction::Shade => self.change_window(window, |target, _| {
                target.set_shaded(!target.state().is_shaded);
            }),
            WindowManagerAction::Stick => self.toggle_sticky(window),
            WindowManagerAction::Move(distance) => {
                self.change_window(window, |target, metrics| target.move_by(*distance, metrics));
            }
            WindowManagerAction::MoveTo(position) => {
                self.change_window(window, |target, metrics| {
                    let corner = target.outer_area(metrics).loc;
                    target.move_by(*position - corner, metrics);
                });
            }
            WindowManagerAction::Resize(growth) => {
                self.change_window(window, |target, metrics| {
                    let size = target.content_area().size;
                    let grown = ((size.w + growth.x).max(1), (size.h + growth.y).max(1));
                    target.resize_to(grown.into(), metrics);
                });
            }
            WindowManagerAction::ResizeTo(size) => {
                self.change_window(window, |target, metrics| target.resize_to(*size, metrics));
            }
            WindowManagerAction::Workspace(target) => {
                if let Some(workspace) = self.workspaces.resolve(*target) {
                    self.show(workspace);
                    self.focus_most_recent();
                }
            }
            WindowManagerAction::SendToWorkspace(target) => {
                self.send_to_workspace(window, *target);
            }
            WindowManagerAction::TakeToWorkspace(target) => {
                if let Some(workspace) = self.send_to_workspace(window, *target) {
                    self.show(workspace);
                    self.focus_most_recent();
                }
            }
            WindowManagerAction::AddWorkspace => {
                let added = self.workspaces.add();
                tracing::debug!(workspace = %added, "workspace added");
            }
            WindowManagerAction::RemoveLastWorkspace => self.remove_last_workspace(),
            WindowManagerAction::NextWindow(cycle) => {
                self.cycle_focus(cycle, CycleDirection::Forward);
            }
            WindowManagerAction::PreviousWindow(cycle) => {
                self.cycle_focus(cycle, CycleDirection::Backward);
            }
            WindowManagerAction::ShowDesktop => self.show_desktop(),
            WindowManagerAction::SetTitle(title) => self.set_title(window, title.clone()),
            WindowManagerAction::SetLayer(layer) => self.set_layer(window, *layer),
            WindowManagerAction::SetAlpha(change) => self.change_alpha(window, *change),
            WindowManagerAction::FocusDirection(direction) => self.focus_direction(*direction),
            WindowManagerAction::MarkWindow(key) => self.mark(window, key),
            WindowManagerAction::SendToOutput(target) => self.send_to_output(window, *target),
            WindowManagerAction::Arrange(style) => self.arrange(window, *style),
            WindowManagerAction::RenameWorkspace(name) => {
                self.workspaces.rename_current(name);
                self.is_redraw_needed = true;
            }
            WindowManagerAction::ToggleFrame => {
                self.change_window(window, |target, _| target.toggle_frame());
                // Its content stays where it is, or fills its place again.
                self.refit_placed_windows();
            }
            WindowManagerAction::GoToMarkedWindow(key) => self.go_to_marked(key),
        }
    }

    /// Moves `window` to the output `target` names: a floating window to
    /// the same place in proportion, a maximized or tiled one filling its
    /// part of the new output.
    fn send_to_output(&mut self, window: Option<WindowId>, target: OutputTarget) {
        let outputs = self.outputs.clone();
        self.change_window(window, |window, metrics| {
            let outer = window.outer_area(metrics);
            let from = outputs.for_area(outer);
            let Some(to) = outputs.target(from, target) else {
                return;
            };
            let placement = window.placement().clone();
            if placement == Placement::Floating {
                let corner = OutputAreas::carried(outer, from, to);
                window.move_by(corner - outer.loc, metrics);
            } else {
                window.place(placement, to, metrics);
            }
        });
        self.is_redraw_needed = true;
    }

    /// Puts `window` back on the output covering `to`, its corner at
    /// `corner` if it floats; maximized or tiled, it fills its part there.
    pub(crate) fn return_to(
        &mut self,
        window: WindowId,
        to: OutputArea,
        corner: Point<i32, Logical>,
    ) {
        self.change_window(Some(window), |window, metrics| {
            let placement = window.placement().clone();
            if placement == Placement::Floating {
                let outer = window.outer_area(metrics);
                window.move_by(corner - outer.loc, metrics);
            } else {
                window.place(placement, to, metrics);
            }
        });
        self.is_redraw_needed = true;
    }

    /// Tiles the windows shown on the output of `window` (or the primary
    /// one) as `style` arranges them, in the order they opened.
    fn arrange(&mut self, window: Option<WindowId>, style: ArrangeStyle) {
        let metrics = self.metrics;
        let output = window.and_then(|window| self.window(window)).map_or_else(
            || self.outputs.iter().next().copied(),
            |window| Some(self.outputs.for_area(window.outer_area(metrics))),
        );
        let Some(output) = output else {
            return;
        };
        let mut windows: Vec<WindowId> = self
            .shown_top_down()
            .filter(|shown| shown.is_placed() && !shown.placement().is_fullscreen())
            .filter(|shown| self.outputs.for_area(shown.outer_area(metrics)) == output)
            .map(Window::id)
            .collect();
        windows.sort_unstable();
        let cells = arrange::cells(windows.len(), style);
        for (window, cell) in windows.into_iter().zip(cells) {
            self.change_placement(Some(window), |placement, current| Placement::Tiled {
                cell,
                restore: placement.restore_area(current),
            });
        }
    }

    /// Marks `window` with `key`, which then marks it alone.
    fn mark(&mut self, window: Option<WindowId>, key: &str) {
        if let Some(window) = window {
            self.marks
                .retain(|(known, marked)| *marked != window && known != key);
            self.marks.push((key.to_owned(), window));
        }
    }

    /// Brings forward the window `key` marks, if one is.
    fn go_to_marked(&mut self, key: &str) {
        let marked = self
            .marks
            .iter()
            .find(|(known, _)| known == key)
            .map(|(_, window)| *window);
        if let Some(window) = marked {
            self.activate(window);
        }
    }

    /// Focuses and raises the nearest window shown in `direction` from the
    /// focused one.
    fn focus_direction(&mut self, direction: Direction) {
        let Some(from) = self.focused().and_then(|focused| self.window(focused)) else {
            return;
        };
        let (from_id, from_area) = (from.id(), from.content_area());
        let shown = self
            .shown_top_down()
            .filter(|window| window.id() != from_id)
            .map(|window| (window.id(), window.content_area()));
        if let Some(window) = direction::nearest_in(from_area, direction, shown) {
            self.focus(Some(window));
            self.raise(window);
        }
    }

    /// Honors what a client asks for `window`, if it changes anything;
    /// returns whether it did. A client asked nothing new must still hear
    /// back, so the compositor answers it when this returns false.
    pub(crate) fn honor(&mut self, window: WindowId, request: ClientRequest) -> bool {
        let Some(placement) = self.windows.get(window).map(Window::placement) else {
            return false;
        };
        let Some(target) = self.windows.get(window) else {
            return false;
        };
        let is_refused = match request {
            ClientRequest::Maximize => target.is_exempt(Exemption::ClientMaximize),
            ClientRequest::Fullscreen => target.is_exempt(Exemption::ClientFullscreen),
            ClientRequest::Unmaximize | ClientRequest::Unfullscreen | ClientRequest::Minimize => {
                false
            }
        };
        if is_refused {
            return false;
        }
        let applies = match request {
            ClientRequest::Maximize => !placement.is_fully_maximized(),
            ClientRequest::Unmaximize => placement.is_fully_maximized(),
            ClientRequest::Fullscreen => !placement.is_fullscreen(),
            ClientRequest::Unfullscreen => placement.is_fullscreen(),
            ClientRequest::Minimize => true,
        };
        if applies {
            let window = Some(window);
            match request {
                ClientRequest::Maximize | ClientRequest::Unmaximize => {
                    self.change_placement(window, |placement, current| {
                        placement.with_maximized_toggled(Axes::Both, current)
                    });
                }
                ClientRequest::Fullscreen | ClientRequest::Unfullscreen => {
                    self.change_placement(window, Placement::with_fullscreen_toggled);
                }
                ClientRequest::Minimize => self.minimize(window),
            }
        }
        applies
    }

    /// Ends what lasts while modifiers are held, now that none is: window
    /// cycling, and undoing a snap step's minimize.
    pub(crate) fn modifiers_released(&mut self) {
        self.focus.finish_cycle();
        self.snap_minimized = None;
    }

    /// Ends window cycling, as a click does.
    pub(crate) fn finish_cycle(&mut self) {
        self.focus.finish_cycle();
    }

    /// Starts `drag` on `window` with the pointer at `pointer`; returns
    /// whether the window exists.
    pub(crate) fn begin_drag(
        &mut self,
        window: WindowId,
        drag: WindowDrag,
        pointer: Point<f64, Logical>,
    ) -> bool {
        let (metrics, style) = (self.metrics, self.resize_style);
        let Some(target) = self.window_mut(window) else {
            return false;
        };
        target.begin_drag(drag, pointer, style, metrics);
        tracing::debug!(%window, ?drag, "drag started");
        true
    }

    /// Follows the drag of `window` after the pointer moved `delta` since
    /// it began; a move snaps to edges within `snap_distance` pixels.
    pub(crate) fn follow_drag(
        &mut self,
        window: WindowId,
        delta: Point<i32, Logical>,
        snap_distance: u16,
    ) {
        let snap = self.edge_snap_for(window, snap_distance);
        let metrics = self.metrics;
        if let Some(target) = self.window_mut(window) {
            target.follow_drag(delta, &snap, metrics);
        }
        // An outline moves without the window changing.
        self.is_redraw_needed = true;
    }

    /// Ends the drag of `window`: a move let go over a snap zone, or a
    /// cell of the layout picker, puts the window there, to be restored to
    /// where the move began.
    pub(crate) fn end_drag(&mut self, window: WindowId) {
        let snap = self
            .drag_snap
            .take()
            .filter(|snap| snap.window == window)
            .and_then(|snap| snap.target);
        if let Some(target) = self.window_mut(window) {
            match snap {
                Some(_) => target.cancel_move(),
                None => target.end_drag(),
            }
            tracing::debug!(%window, ?snap, "drag ended");
        }
        match snap {
            Some(SnapTarget::Maximize) => {
                self.change_placement(Some(window), |placement, current| match placement {
                    Placement::Floating => placement.with_maximized_toggled(Axes::Both, current),
                    other @ (Placement::Maximized { .. }
                    | Placement::Tiled { .. }
                    | Placement::Fullscreen { .. }) => other,
                });
            }
            Some(SnapTarget::Cell(cell)) => {
                self.change_placement(Some(window), |_, current| Placement::Tiled {
                    cell,
                    restore: current,
                });
            }
            None => {}
        }
        self.is_redraw_needed = true;
    }

    /// Follows the pointer, at `pointer`, of a move of `window`: over a
    /// snap zone or the layout picker's cells, the window would go there.
    pub(crate) fn follow_drag_pointer(&mut self, window: WindowId, pointer: Point<i32, Logical>) {
        let is_moving = self.windows.get(window).is_some_and(Window::is_moving);
        if !self.snapping.enabled || !is_moving {
            self.drag_snap = None;
            return;
        }
        let usable = self.outputs.at(pointer).usable;
        let distance = i32::from(self.snapping.zone_distance);
        let zone = SnapTarget::at(pointer, usable, distance);
        let was_picking = self
            .drag_snap
            .as_ref()
            .is_some_and(|snap| snap.picker.is_some());
        let mut target = zone;
        let mut picker = None;
        if self.snapping.layout_picker
            && let Some(layout) = LayoutPicker::new(
                self.snapping.layouts.len(),
                usable,
                self.snapping.thumbnail_size(),
                self.snapping.picker_columns,
                distance,
            )
            && (zone == Some(SnapTarget::Maximize)
                || (was_picking && layout.holds(pointer, usable)))
        {
            let hit = layout.hit(&self.snapping.layouts, pointer);
            if let Some((index, cell)) = hit {
                target = Some(SnapTarget::Cell(self.snapping.layouts[index][cell]));
            } else if zone != Some(SnapTarget::Maximize) {
                // Between the picker's cells: nowhere.
                target = None;
            }
            picker = Some((layout, hit));
        }
        let snap = DragSnap {
            window,
            target,
            usable,
            picker,
        };
        if self.drag_snap.as_ref() != Some(&snap) {
            self.drag_snap = Some(snap);
            self.is_redraw_needed = true;
        }
    }

    /// Returns where the window being moved would snap, with its id.
    pub(crate) fn snap_preview(&self) -> Option<(WindowId, Area)> {
        let snap = self.drag_snap.as_ref()?;
        Some((snap.window, snap.target?.area_in(snap.usable)))
    }

    /// Returns the layout picker shown, the layouts and the cell under the
    /// pointer, while a move shows it.
    pub(crate) fn layout_picker(&self) -> Option<ShownPicker<'_>> {
        let (picker, hovered) = self.drag_snap.as_ref()?.picker.as_ref()?;
        Some(ShownPicker {
            picker,
            layouts: &self.snapping.layouts,
            hovered: *hovered,
        })
    }

    /// Holds down the titlebar button of `window` at `location`, if there
    /// is one; returns whether there was.
    pub(crate) fn press_titlebar_button(
        &mut self,
        window: WindowId,
        location: Point<f64, Logical>,
    ) -> bool {
        let metrics = self.metrics;
        let is_pressed = self
            .window_mut(window)
            .is_some_and(|target| target.press_titlebar_button(location, metrics));
        if is_pressed {
            self.held_titlebar = Some(window);
        }
        is_pressed
    }

    /// Notes the pointer is on a window's titlebar at `at` (a window and the
    /// pointer's place), or on none; the button it is on shows it. Returns
    /// whether anything shown changed.
    pub(crate) fn hover_titlebar(&mut self, at: Option<(WindowId, Point<f64, Logical>)>) -> bool {
        let metrics = self.metrics;
        let mut changed = false;
        if let Some(previous) = self.hovered_titlebar
            && at.map(|(window, _)| window) != Some(previous)
        {
            changed |= self
                .window_mut(previous)
                .is_some_and(|target| target.hover_titlebar_button(None, metrics));
            self.hovered_titlebar = None;
        }
        if let Some((window, location)) = at
            && let Some(target) = self.window_mut(window)
        {
            changed |= target.hover_titlebar_button(Some(location), metrics);
            self.hovered_titlebar = Some(window);
        }
        changed
    }

    /// Lets go of a held titlebar button with the pointer at `location`,
    /// running its action if the pointer is still on it. Returns the window
    /// whose menu button was clicked, which is the compositor's to open.
    pub(crate) fn release_titlebar_button(
        &mut self,
        location: Point<f64, Logical>,
    ) -> Option<WindowId> {
        let window = self.held_titlebar.take()?;
        let metrics = self.metrics;
        let clicked = self
            .window_mut(window)
            .and_then(|target| target.release_titlebar_button(location, metrics));
        match clicked?.action() {
            Some(action) => {
                tracing::debug!(%action, %window, "titlebar button clicked");
                self.run(&action, Some(window));
                None
            }
            None => Some(window),
        }
    }

    /// Brings what follows the settings up to date with them: the
    /// workspaces' names and number, and what frames show.
    pub(crate) fn apply_settings(&mut self, settings: &Settings) {
        if let Some(last) = self.workspaces.rename(settings.workspaces.names()) {
            for window in self.windows.iter_mut() {
                if window.workspace() > last {
                    window.move_to_workspace(last);
                }
            }
        }
        let buttons = settings.windows.frame.titlebar_layout();
        for window in self.windows.iter_mut() {
            window.set_titlebar_buttons(&buttons);
        }
        self.resize_style = Self::resize_style(settings);
        self.default_alpha = settings.windows.alpha();
        self.snapping = settings.windows.snapping.clone();
        if settings.windows.frame != self.frame {
            self.frame = settings.windows.frame.clone();
            self.restyle(self.style_metrics);
        }
        // A window is chosen to focus only where the focused one was taken
        // away (its workspace went); a reload never gives focus to a window
        // when none had it.
        if self.focused().is_some() {
            self.focus_most_recent();
        }
        self.is_redraw_needed = true;
    }

    /// Places windows on `outputs` from now on; maximized, tiled and
    /// fullscreen windows follow at once, as when a panel appears.
    pub(crate) fn set_output_areas(&mut self, outputs: OutputAreas) {
        if outputs == self.outputs {
            return;
        }
        self.outputs = outputs;
        self.refit_placed_windows();
    }

    /// Lays windows out with frames sized by `metrics` from now on, and
    /// paints every frame again: the style changed. A floating window's
    /// content stays where it is; the others are placed again.
    pub(crate) fn restyle(&mut self, metrics: Metrics) {
        self.style_metrics = metrics;
        self.metrics = self.frame.frame_metrics(metrics);
        for window in self.windows.iter_mut() {
            window.invalidate_frame();
        }
        self.refit_placed_windows();
    }

    /// Paints every window's frame that changed.
    pub(crate) fn paint_frames(
        &mut self,
        style: &Style,
        (typesetter, icons): (&mut Typesetter, &mut IconStore),
    ) {
        let metrics = self.metrics;
        let icon_size = WidgetIcon::side(metrics.titlebar_height);
        let shows_icons = self.frame.icon && icon_size > 0;
        for window in self.windows.iter_mut() {
            window.show_frame_icon(shows_icons.then_some(&mut *icons), icon_size);
            window.paint_frame(metrics, style, typesetter);
        }
    }

    /// Returns what clients must be told since the last call.
    pub(crate) fn take_client_updates(&mut self) -> ClientUpdates {
        ClientUpdates {
            states: self
                .windows
                .iter_mut()
                .filter_map(|window| Some((window.id(), window.take_request()?)))
                .collect(),
            endings: std::mem::take(&mut self.endings),
        }
    }

    /// Returns whether anything shown changed since the last call.
    pub(crate) const fn take_redraw(&mut self) -> bool {
        std::mem::replace(&mut self.is_redraw_needed, false)
    }

    /// Places maximized, tiled and fullscreen windows again, each in its
    /// output's areas and with frames sized as they are now; a floating
    /// window no output shows any more, its output gone, moves onto the
    /// primary one.
    fn refit_placed_windows(&mut self) {
        let metrics = self.metrics;
        for window in self.windows.iter_mut() {
            if !window.is_placed() {
                continue;
            }
            let outer = window.outer_area(metrics);
            let placement = window.placement().clone();
            if placement == Placement::Floating {
                let corner = self.outputs.corner_shown(outer);
                if corner != outer.loc {
                    window.move_by(corner - outer.loc, metrics);
                }
            } else {
                window.place(placement, self.outputs.for_area(outer), metrics);
            }
        }
        self.is_redraw_needed = true;
    }

    /// Returns the window with `id` to change it; the change shows.
    fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        let window = self.windows.get_mut(id)?;
        self.is_redraw_needed = true;
        Some(window)
    }

    /// Gives the window `id` its first place, by `config`'s placement
    /// settings and rules, among the windows already shown, on the output
    /// the `pointer` is on.
    fn place_new_window(&mut self, id: WindowId, config: &Config, pointer: Point<i32, Logical>) {
        let (metrics, current) = (self.metrics, self.workspaces.current());
        let Some(window) = self.windows.get(id) else {
            return;
        };
        let rule = config.rules.for_window(window, current);
        let ruled_output = rule.output.as_ref().and_then(|output| match output {
            RuleOutput::Head(number) => self.outputs.head(*number),
            RuleOutput::Named(name) => self.outputs.by_name(name),
        });
        let output = ruled_output.unwrap_or_else(|| self.outputs.at(pointer));
        // Before its size is read: the frame is part of it.
        if let Some(frame) = rule.frame
            && let Some(window) = self.window_mut(id)
        {
            window.set_framed(frame == RuleFrame::Full);
        }
        let room = Room {
            usable: output.usable,
            occupied: self
                .windows
                .shown_top_down(current)
                .filter(|window| window.id() != id && window.is_placed())
                .map(|window| window.outer_area(metrics))
                .collect(),
            pointer,
        };
        let Some(window) = self.windows.get(id) else {
            return;
        };
        let size = rule.size().unwrap_or(window.outer_area(metrics).size);
        // A dialog opens over its parent, unless a rule places it.
        let over_parent = ruled_output
            .is_none()
            .then(|| self.dialog_area(id, size))
            .flatten();
        let corner = match (rule.position, over_parent) {
            (Some(position), _) => position.corner(room.usable, size),
            (None, Some(over_parent)) => over_parent.loc,
            (None, None) => config.settings.placement.place(size, &room),
        };
        let outer = Rectangle::new(corner, size);
        if let Some(window) = self.window_mut(id) {
            window.place_initially(outer, metrics);
        }
        self.apply_rule(id, &rule, outer, output);
        // Its first focus too: Fluxbox's [FocusProtection] {Gain} and
        // {Refuse} decide it over focusNewWindows.
        match rule.focus_stealing {
            Some(FocusStealing::Allow) => self.focus(Some(id)),
            Some(FocusStealing::Deny) => self.give_focus_back(id),
            Some(FocusStealing::Default) | None => {}
        }
        // A rule may have sent the new window out of sight.
        self.focus_most_recent();
    }

    /// Gives the window `id`, just placed at `outer` on `output`, the
    /// states and traits `rule` says.
    fn apply_rule(&mut self, id: WindowId, rule: &RuleSettings, outer: Area, output: OutputArea) {
        let metrics = self.metrics;
        let workspace = rule
            .workspace
            .map(Workspace::numbered)
            .filter(|workspace| self.workspaces.contains(*workspace));
        if let Some(layer) = rule.layer {
            self.windows.set_layer(id, layer);
        }
        let Some(window) = self.window_mut(id) else {
            return;
        };
        if let Some(workspace) = workspace {
            window.move_to_workspace(workspace);
        }
        if let Some(is_sticky) = rule.sticky {
            window.set_sticky(is_sticky);
        }
        if let Some(is_minimized) = rule.minimized {
            window.set_minimized(is_minimized);
        }
        if let Some(placement) = rule.placement {
            window.place(placement.placement(outer), output, metrics);
        }
        if let Some(is_pseudo_transparent) = rule.pseudo_transparent {
            window.set_pseudo_transparent(is_pseudo_transparent);
        }
        if let Some(is_shaded) = rule.shaded {
            window.set_shaded(is_shaded);
        }
        if let Some(alpha) = rule.alpha {
            window.set_alpha(alpha.alpha());
        }
        for (exemption, is_exempt) in &rule.exemptions {
            window.set_exempt(*exemption, *is_exempt);
        }
        if let Some(focus_stealing) = rule.focus_stealing {
            window.set_focus_stealing(focus_stealing);
        }
    }

    /// Gives every open window the traits `config`'s rules say now: frame,
    /// layer, stickiness, opacity, pseudo-transparency, exemptions and
    /// focus stealing. Where a window goes and how large it is, its
    /// workspace and its state are for a window opening, and are left.
    pub(crate) fn reapply_rule_traits(&mut self, config: &Config) {
        let current = self.workspaces.current();
        let ids: Vec<WindowId> = self.windows.top_down().map(Window::id).collect();
        for id in ids {
            let Some(window) = self.windows.get(id) else {
                continue;
            };
            let rule = config.rules.for_window(window, current);
            if let Some(layer) = rule.layer {
                self.windows.set_layer(id, layer);
            }
            let Some(window) = self.window_mut(id) else {
                continue;
            };
            if let Some(frame) = rule.frame {
                window.set_framed(frame == RuleFrame::Full);
            }
            if let Some(is_sticky) = rule.sticky {
                window.set_sticky(is_sticky);
            }
            if let Some(is_pseudo_transparent) = rule.pseudo_transparent {
                window.set_pseudo_transparent(is_pseudo_transparent);
            }
            if let Some(alpha) = rule.alpha {
                window.set_alpha(alpha.alpha());
            }
            for (exemption, is_exempt) in &rule.exemptions {
                window.set_exempt(*exemption, *is_exempt);
            }
            if let Some(focus_stealing) = rule.focus_stealing {
                window.set_focus_stealing(focus_stealing);
            }
        }
        // A frame may have come or gone: content stays, or fills its place.
        self.refit_placed_windows();
    }

    /// Moves `window`, floating, so its content's corner is at `corner`;
    /// returns whether it moved.
    pub(crate) fn move_content_to(
        &mut self,
        window: WindowId,
        corner: Point<i32, Logical>,
    ) -> bool {
        let metrics = self.metrics;
        let Some(target) = self.window_mut(window) else {
            return false;
        };
        let delta = corner - target.content_area().loc;
        if delta == Point::default() {
            return false;
        }
        target.move_by(delta, metrics);
        self.is_redraw_needed = true;
        true
    }

    /// Applies `change` to `window`, given the style's metrics.
    fn change_window(
        &mut self,
        window: Option<WindowId>,
        change: impl FnOnce(&mut Window, Metrics),
    ) {
        let metrics = self.metrics;
        if let Some(target) = window.and_then(|id| self.window_mut(id)) {
            change(target, metrics);
        }
    }

    /// Places `window` where `change` says, given its placement and the
    /// area it covers now.
    fn change_placement(
        &mut self,
        window: Option<WindowId>,
        change: impl FnOnce(Placement, Area) -> Placement,
    ) {
        let outputs = self.outputs.clone();
        self.change_window(window, |target, metrics| {
            let current = target.outer_area(metrics);
            let placement = change(target.placement().clone(), current);
            target.place(placement, outputs.for_area(current), metrics);
        });
    }

    /// Takes a step of the snap ladder with `window`. A step up right after
    /// a step down minimized a window, its modifiers still held, restores
    /// it; anything else then does nothing, since the window it would act
    /// on is out of sight.
    fn snap_step(&mut self, window: Option<WindowId>, direction: Direction) {
        if let Some(minimized) = self.snap_minimized {
            if direction == Direction::Up {
                self.snap_minimized = None;
                self.activate(minimized);
            }
            return;
        }
        let metrics = self.metrics;
        let Some(target) = window.and_then(|id| self.windows.get(id)) else {
            return;
        };
        let current = target.outer_area(metrics);
        let usable = self.outputs.for_area(current).usable;
        match target
            .placement()
            .clone()
            .snap_step(direction, current, usable)
        {
            SnapStep::Place(placement) => self.change_placement(window, |_, _| placement),
            SnapStep::Minimize => {
                self.minimize(window);
                self.snap_minimized = window;
            }
            SnapStep::Unchanged => {}
        }
    }

    /// Minimizes `window`; focus goes to the window that had it before.
    fn minimize(&mut self, window: Option<WindowId>) {
        if let Some(window) = window {
            for member in self.family_bottom_up(window) {
                self.change_window(Some(member), |target, _| target.set_minimized(true));
            }
        }
        self.focus_most_recent();
    }

    /// Shows `window` on every workspace, or, when it already is, only on
    /// the current one.
    fn toggle_sticky(&mut self, window: Option<WindowId>) {
        let current = self.workspaces.current();
        self.change_window(window, |target, _| {
            let is_sticky = !target.state().is_sticky;
            target.set_sticky(is_sticky);
            if !is_sticky {
                target.move_to_workspace(current);
            }
        });
    }

    /// Shows `workspace`.
    fn show(&mut self, workspace: Workspace) {
        if self.workspaces.show(workspace) {
            tracing::debug!(%workspace, "workspace shown");
            self.is_redraw_needed = true;
        }
    }

    /// Puts `window` on the workspace `target` means; returns that
    /// workspace, if it exists.
    fn send_to_workspace(
        &mut self,
        window: Option<WindowId>,
        target: WorkspaceTarget,
    ) -> Option<Workspace> {
        let workspace = self.workspaces.resolve(target)?;
        self.change_window(window, |target, _| target.move_to_workspace(workspace));
        self.focus_most_recent();
        Some(workspace)
    }

    /// Removes the last workspace; its windows move to the one before.
    fn remove_last_workspace(&mut self) {
        let Some(removed) = self.workspaces.remove_last() else {
            return;
        };
        let last = self.workspaces.last();
        for window in self.windows.iter_mut() {
            if window.workspace() == removed {
                window.move_to_workspace(last);
            }
        }
        tracing::debug!(workspace = %removed, "workspace removed");
        self.focus_most_recent();
        self.is_redraw_needed = true;
    }

    /// Focuses and raises the next window in `direction` among those the
    /// cycle's matcher matches, in its order. Minimized windows count only
    /// if the matcher asks for them.
    fn cycle_focus(&mut self, cycle: &WindowCycle, direction: CycleDirection) {
        let current = self.workspaces.current();
        let windows = &cycle.windows;
        let mut candidates = self
            .windows
            .top_down()
            .filter(|window| {
                (windows.admits_minimized() || !window.state().is_minimized)
                    && !window.is_exempt(Exemption::Cycling)
                    && windows.matches(window, current)
            })
            .map(Window::id)
            .collect::<Vec<_>>();
        let next = match cycle.order {
            CycleOrder::Focus => {
                let ordered = self.focus.by_recency(candidates);
                self.focus.step(ordered, direction)
            }
            CycleOrder::Opening => {
                candidates.sort();
                Self::step_from(&candidates, self.focused(), direction)
            }
        };
        if let Some(next) = next {
            self.activate(next);
        }
    }

    /// Returns the window a step in `direction` from `from` goes to among
    /// `windows`, wrapping around; the first or last when `from` is not
    /// among them.
    fn step_from(
        windows: &[WindowId],
        from: Option<WindowId>,
        direction: CycleDirection,
    ) -> Option<WindowId> {
        let count = windows.len();
        let position = from.and_then(|from| windows.iter().position(|id| *id == from));
        let index = match (position, direction) {
            (_, _) if count == 0 => return None,
            (Some(position), CycleDirection::Forward) => (position + 1) % count,
            (Some(position), CycleDirection::Backward) => (position + count - 1) % count,
            (None, CycleDirection::Forward) => 0,
            (None, CycleDirection::Backward) => count - 1,
        };
        windows.get(index).copied()
    }

    /// Minimizes every window shown, or, when none is, restores the
    /// minimized windows of the current workspace.
    fn show_desktop(&mut self) {
        let current = self.workspaces.current();
        let is_desktop_shown = self.windows.shown_top_down(current).next().is_none();
        for window in self.windows.iter_mut() {
            if window.state().is_sticky || window.workspace() == current {
                window.set_minimized(!is_desktop_shown);
            }
        }
        self.focus_most_recent();
        self.is_redraw_needed = true;
    }

    /// Asks `window` to end as `ending` says.
    fn end(&mut self, window: Option<WindowId>, ending: Ending) {
        if let Some(window) = window.filter(|id| self.windows.get(*id).is_some()) {
            self.endings.push((window, ending));
        }
    }

    /// Takes focus from `window`, a new window whose rule refuses it, and
    /// gives it back to the most recently focused other window shown.
    fn give_focus_back(&mut self, window: WindowId) {
        if self.focused() != Some(window) {
            return;
        }
        self.focus.forget(window);
        let current = self.workspaces.current();
        let others = self
            .windows
            .shown_top_down(current)
            .map(Window::id)
            .filter(|id| *id != window)
            .collect::<Vec<_>>();
        let most_recent = self.focus.by_recency(others).first().copied();
        self.focus(most_recent);
    }

    /// Gives focus to the window that had it most recently among those
    /// shown, if the focused one is not shown: it was minimized, closed,
    /// or left on another workspace.
    fn focus_most_recent(&mut self) {
        let current = self.workspaces.current();
        let is_shown = |id| {
            self.windows
                .get(id)
                .is_some_and(|window| window.is_shown_on(current))
        };
        if self.focused().is_some_and(is_shown) {
            return;
        }
        let shown = self
            .windows
            .shown_top_down(current)
            .map(Window::id)
            .collect::<Vec<_>>();
        let most_recent = self.focus.by_recency(shown).first().copied();
        self.focus(most_recent);
    }

    /// Returns the edges a move of `window` snaps to within `distance`
    /// pixels: the outputs' usable areas', and the other shown windows'.
    fn edge_snap_for(&self, window: WindowId, distance: u16) -> EdgeSnap {
        let others = self
            .shown_top_down()
            .filter(|other| other.id() != window)
            .map(|other| other.outer_area(self.metrics))
            .collect();
        let usable = self.outputs.iter().map(|output| output.usable).collect();
        EdgeSnap::new(distance, usable, others)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use proptest::prelude::*;
    use rstest::rstest;
    use smithay::utils::Size;

    use super::*;
    use crate::config::{Action, Rules};

    /// A titlebar 20 pixels tall, lines 1 pixel wide and a handle 1 tall.
    const METRICS: Metrics = Metrics {
        titlebar_height: 20,
        border_width: 1,
        handle_height: 1,
        taskbar_height: 20,
        frame_corners: vaht_ui::CornerRadii::SQUARE,
        menu: vaht_ui::style::MenuMetrics {
            title_height: 20,
            item_height: 20,
            border_width: 1,
            corners: vaht_ui::CornerRadii::SQUARE,
            bullet: vaht_ui::style::Bullet::Triangle,
            bullet_side: vaht_ui::MarkSide::Right,
        },
    };

    /// A 1920 × 1080 output, `panel` pixels of it at the bottom taken by a
    /// panel.
    fn output(panel: i32) -> OutputArea {
        let whole = Rectangle::from_size((1920, 1080).into());
        let mut usable = whole;
        usable.size.h -= panel;
        OutputArea { whole, usable }
    }

    /// Only the output [`output`] makes.
    fn only(panel: i32) -> OutputAreas {
        OutputAreas::new(output(panel), Vec::new())
    }

    /// A window manager with the built-in configuration, whose clients do
    /// exactly what they are asked.
    struct Desk {
        /// The window manager.
        manager: WindowManager,
        /// The configuration it is given.
        config: Config,
        /// Counts what it creates.
        ledger: ResourceLedger,
        /// What each client last committed.
        committed: HashMap<WindowId, CommittedState>,
        /// Every ending asked for, in order.
        endings: Vec<(WindowId, Ending)>,
    }

    impl Desk {
        /// An empty desk with the built-in configuration, but frames that
        /// show the handle, so every part of a frame is laid out.
        fn new() -> Self {
            let mut config = Config::built_in();
            config.settings.windows.frame.handle = true;
            Self::with_config(config)
        }

        /// An empty desk with `config`.
        fn with_config(config: Config) -> Self {
            Self {
                manager: WindowManager::new(&config.settings, only(0), METRICS),
                config,
                ledger: ResourceLedger::new(),
                committed: HashMap::new(),
                endings: Vec::new(),
            }
        }

        /// Opens a window of `app_id` whose client draws content of `size`,
        /// and lets it be placed.
        fn open_app(&mut self, app_id: &str, size: (i32, i32)) -> WindowId {
            let window = self.manager.add_window(&self.config.settings, &self.ledger);
            let committed = CommittedState {
                title: format!("window {window}"),
                app_id: app_id.to_owned(),
                app_id_alternate: String::new(),
                icon: None,
                geometry: Rectangle::from_size(size.into()),
                size_limits: SizeLimits::default(),
                is_self_decorated: false,
            };
            self.commit(window, committed);
            self.settle();
            window
        }

        /// Opens a window of 400 × 300 content that draws its own frame.
        fn open_self_decorated(&mut self) -> WindowId {
            let window = self.manager.add_window(&self.config.settings, &self.ledger);
            let committed = CommittedState {
                title: format!("window {window}"),
                app_id: "self".to_owned(),
                app_id_alternate: String::new(),
                icon: None,
                geometry: Rectangle::from_size((400, 300).into()),
                size_limits: SizeLimits::default(),
                is_self_decorated: true,
            };
            self.commit(window, committed);
            self.settle();
            window
        }

        /// Opens a window of 400 × 300 content.
        fn open(&mut self) -> WindowId {
            self.open_app("app", (400, 300))
        }

        /// Opens a dialog of `parent`, modal if `is_modal`, of 200 × 100
        /// content.
        fn open_dialog(&mut self, parent: WindowId, is_modal: bool) -> WindowId {
            let window = self.manager.add_window(&self.config.settings, &self.ledger);
            // As the compositor does: the family before the first commit.
            self.manager.set_family(window, Some(parent), is_modal);
            let committed = CommittedState {
                title: format!("dialog {window}"),
                app_id: "app".to_owned(),
                app_id_alternate: String::new(),
                icon: None,
                geometry: Rectangle::from_size((200, 100).into()),
                size_limits: SizeLimits::default(),
                is_self_decorated: false,
            };
            self.commit(window, committed);
            self.settle();
            window
        }

        /// Closes `window`, as its client does.
        fn close(&mut self, window: WindowId) {
            self.committed.remove(&window);
            self.manager.remove_window(window);
        }

        /// Runs `action` on `window`, and lets clients answer.
        fn run_on(&mut self, action: &WindowManagerAction, window: Option<WindowId>) {
            self.manager.run(action, window);
            self.settle();
        }

        /// Runs `action` on the focused window, as a key binding does.
        fn run(&mut self, action: &WindowManagerAction) {
            self.run_on(action, self.manager.focused());
        }

        /// Lets every client do what it was asked, until nothing new is.
        fn settle(&mut self) {
            loop {
                let updates = self.manager.take_client_updates();
                if updates == ClientUpdates::default() {
                    return;
                }
                self.endings.extend(updates.endings);
                for (window, requested) in updates.states {
                    let (Some(size), Some(committed)) =
                        (requested.size, self.committed.get(&window).cloned())
                    else {
                        continue;
                    };
                    let mut committed = committed;
                    committed.geometry.size = size;
                    self.commit(window, committed);
                }
            }
        }

        /// Gives the window manager what the client of `window` committed.
        fn commit(&mut self, window: WindowId, committed: CommittedState) {
            self.committed.insert(window, committed.clone());
            let pointer = Point::from((960, 540));
            self.manager
                .commit(window, committed, &self.config, pointer);
        }

        /// Returns the window `id`.
        fn window(&self, id: WindowId) -> &Window {
            self.manager.window(id).expect("the window exists")
        }

        /// Returns the area `window` covers, frame included.
        fn outer(&self, window: WindowId) -> Area {
            self.window(window).outer_area(self.manager.metrics())
        }

        /// Returns the windows shown, from top to bottom.
        fn shown(&self) -> Vec<WindowId> {
            self.manager.shown_top_down().map(Window::id).collect()
        }
    }

    /// The action of `text`, as a binding would give it.
    fn action(text: &str) -> WindowManagerAction {
        match Action::parse(text) {
            Ok(Action::WindowManager(action)) => action,
            other => panic!("`{text}` is a window-management action, not {other:?}"),
        }
    }

    #[test]
    fn a_dialog_opens_centered_over_its_parent() {
        let mut desk = Desk::new();
        desk.open();
        let parent = desk.open();
        let dialog = desk.open_dialog(parent, false);
        let (over, placed) = (desk.outer(parent), desk.outer(dialog));
        let middle = |area: Area| area.loc + area.size.downscale(2).to_point();
        assert!(
            (middle(over) - middle(placed)).x.abs() <= 1
                && (middle(over) - middle(placed)).y.abs() <= 1,
            "{placed:?} is centered over {over:?}"
        );
    }

    #[test]
    fn a_modal_dialog_keeps_the_focus_from_its_parent() {
        let mut desk = Desk::new();
        let parent = desk.open();
        let other = desk.open();
        let dialog = desk.open_dialog(parent, true);
        desk.manager.focus(Some(other));
        desk.manager.focus(Some(parent));
        assert_eq!(desk.manager.focused(), Some(dialog));
        let free = desk.open_dialog(other, false);
        desk.manager.focus(Some(other));
        assert_eq!(
            desk.manager.focused(),
            Some(other),
            "a dialog that is not modal leaves the focus be"
        );
        desk.close(dialog);
        desk.manager.focus(Some(parent));
        assert_eq!(desk.manager.focused(), Some(parent), "once it closes");
        assert!(desk.manager.window(free).is_some());
    }

    #[test]
    fn a_window_and_its_dialogs_are_raised_lowered_and_minimized_together() {
        let mut desk = Desk::new();
        let parent = desk.open();
        let dialog = desk.open_dialog(parent, false);
        let inner = desk.open_dialog(dialog, false);
        let other = desk.open();
        desk.manager.raise(parent);
        assert_eq!(desk.shown(), [inner, dialog, parent, other]);
        desk.manager.lower(parent);
        assert_eq!(desk.shown(), [other, inner, dialog, parent]);
        desk.run_on(&action("minimize"), Some(parent));
        assert_eq!(desk.shown(), [other], "the dialogs go with it");
        desk.manager.activate(parent);
        assert_eq!(
            desk.shown(),
            [inner, dialog, parent, other],
            "and come back"
        );
    }

    #[test]
    fn a_family_has_no_loops_and_outlives_its_parent() {
        let mut desk = Desk::new();
        let parent = desk.open();
        let dialog = desk.open_dialog(parent, true);
        desk.manager.set_family(parent, Some(dialog), false);
        assert_eq!(desk.window(parent).parent(), None, "a loop is ignored");
        desk.close(parent);
        assert_eq!(desk.window(dialog).parent(), None);
        assert!(!desk.window(dialog).is_modal());
    }

    #[test]
    fn new_windows_are_placed_flush_beside_each_other() {
        let mut desk = Desk::new();
        let first = desk.open();
        let second = desk.open();
        assert_eq!(desk.outer(first).loc, Point::from((0, 0)));
        assert_eq!(
            desk.outer(second).loc,
            Point::from((desk.outer(first).size.w, 0)),
            "the row strategy puts it right of the first"
        );
    }

    #[test]
    fn focus_moves_to_the_window_beside_the_focused_one() {
        let mut desk = Desk::new();
        let left = desk.open();
        let right = desk.open();
        assert_eq!(desk.manager.focused(), Some(right), "the newest has focus");
        desk.manager.run(&action("focus-direction left"), None);
        assert_eq!(desk.manager.focused(), Some(left));
        assert_eq!(desk.shown().first(), Some(&left), "and is raised");
        desk.manager.run(&action("focus-direction left"), None);
        assert_eq!(desk.manager.focused(), Some(left), "nothing further left");
        desk.manager.run(&action("focus-direction right"), None);
        assert_eq!(desk.manager.focused(), Some(right));
    }

    #[test]
    fn a_window_sent_to_another_output_keeps_its_place_there() {
        let mut desk = Desk::new();
        let window = desk.open();
        let right = Rectangle::new((1920, 0).into(), (1280, 1024).into());
        desk.manager.set_output_areas(OutputAreas::new(
            output(0),
            vec![OutputArea {
                whole: right,
                usable: right,
            }],
        ));
        desk.run(&action("send-to-output next"));
        assert!(
            right.contains_rect(desk.outer(window)),
            "on the right output"
        );
        desk.run(&action("send-to-output 1"));
        assert_eq!(
            desk.outer(window).loc,
            Point::from((0, 0)),
            "back where it was"
        );
        desk.run(&action("maximize"));
        desk.run(&action("send-to-output next"));
        assert_eq!(desk.outer(window), right, "maximized there");
    }

    #[test]
    fn a_window_shown_without_its_frame_keeps_its_content_still() {
        let mut desk = Desk::new();
        let window = desk.open();
        let content = desk.window(window).content_area();
        desk.run(&action("toggle-frame"));
        assert!(!desk.window(window).has_frame());
        assert_eq!(desk.window(window).content_area(), content);
        assert_eq!(desk.outer(window), content, "nothing around it");
        desk.run(&action("maximize"));
        assert_eq!(
            desk.window(window).content_area(),
            output(0).usable,
            "fills it all"
        );
        desk.run(&action("maximize"));
        desk.run(&action("toggle-frame"));
        assert!(desk.window(window).has_frame());
        assert_eq!(desk.window(window).content_area(), content);
    }

    #[test]
    fn arranging_tiles_the_windows_in_a_grid() {
        let mut desk = Desk::new();
        let first = desk.open();
        let second = desk.open();
        let third = desk.open();
        desk.run(&action("arrange"));
        let usable = output(0).usable;
        assert_eq!(desk.outer(first).loc, usable.loc, "the first top left");
        assert_eq!(
            desk.outer(second).loc.y,
            usable.loc.y,
            "the second beside it"
        );
        assert!(desk.outer(second).loc.x > desk.outer(first).loc.x);
        assert_eq!(
            desk.outer(third).loc.x,
            usable.loc.x,
            "the third below the first"
        );
        assert!(desk.outer(third).loc.y > usable.loc.y);
    }

    #[test]
    fn the_workspace_shown_is_renamed() {
        let mut desk = Desk::new();
        desk.manager.run(&action("workspace 2"), None);
        desk.manager
            .run(&action("rename-workspace Games and more"), None);
        assert_eq!(desk.manager.current_workspace_name(), "Games and more");
        desk.manager.run(&action("workspace 1"), None);
        assert_ne!(desk.manager.current_workspace_name(), "Games and more");
    }

    #[test]
    fn a_marked_window_is_brought_forward_by_its_key() {
        let mut desk = Desk::new();
        let marked = desk.open();
        let other = desk.open();
        desk.manager.run(&action("mark-window e"), Some(marked));
        desk.manager.run(&action("minimize"), Some(marked));
        desk.manager
            .run(&action("go-to-marked-window e"), Some(other));
        assert_eq!(desk.manager.focused(), Some(marked));
        assert!(!desk.window(marked).state().is_minimized, "restored");
        desk.manager.run(&action("mark-window e"), Some(other));
        desk.manager.run(&action("go-to-marked-window e"), None);
        assert_eq!(
            desk.manager.focused(),
            Some(other),
            "a key marks one window"
        );
        desk.manager.remove_window(other);
        desk.manager.run(&action("go-to-marked-window e"), None);
        assert_eq!(
            desk.manager.focused(),
            Some(marked),
            "a closed window's mark is gone"
        );
    }

    #[test]
    fn a_taskbar_click_brings_a_window_forward_or_minimizes_the_focused_one() {
        let mut desk = Desk::new();
        let behind = desk.open();
        let front = desk.open();
        desk.manager.activate_or_minimize(front);
        assert!(desk.window(front).state().is_minimized, "it had focus");
        assert_eq!(desk.manager.focused(), Some(behind));
        desk.manager.activate_or_minimize(front);
        assert!(!desk.window(front).state().is_minimized);
        assert_eq!(desk.manager.focused(), Some(front));
        assert_eq!(desk.shown().first(), Some(&front), "and raised");
    }

    #[test]
    fn closing_the_focused_window_focuses_the_one_before() {
        let mut desk = Desk::new();
        let first = desk.open();
        let second = desk.open();
        assert_eq!(
            desk.manager.focused(),
            Some(second),
            "new windows take focus"
        );
        desk.close(second);
        assert_eq!(desk.manager.focused(), Some(first));
    }

    /// A desk whose rules say `focus_stealing = policy` for app `rude`, and
    /// whose settings say whether new windows take focus.
    fn desk_with_focus_rule(policy: &str, focus_new_windows: bool) -> Desk {
        let mut config = Config::built_in();
        config.settings.focus.focus_new_windows = focus_new_windows;
        config.rules = toml::from_str(&format!(
            "[[rule]]\nmatch = {{ app_id = \"rude\" }}\nfocus_stealing = \"{policy}\"\n"
        ))
        .expect("valid rules");
        Desk::with_config(config)
    }

    #[test]
    fn a_rule_refusing_focus_gives_it_back_to_the_window_that_had_it() {
        let mut desk = desk_with_focus_rule("deny", true);
        let first = desk.open();
        let rude = desk.open_app("rude", (300, 200));
        assert_ne!(desk.manager.focused(), Some(rude));
        assert_eq!(desk.manager.focused(), Some(first));
        let polite = desk.open();
        assert_eq!(desk.manager.focused(), Some(polite), "others still take it");
    }

    #[test]
    fn a_rule_allowing_focus_takes_it_when_new_windows_do_not() {
        let mut desk = desk_with_focus_rule("allow", false);
        let first = desk.open();
        let rude = desk.open_app("rude", (300, 200));
        assert_eq!(desk.manager.focused(), Some(rude), "taken from {first}");
        desk.open();
        assert_eq!(desk.manager.focused(), Some(rude), "others do not take it");
    }

    #[test]
    fn window_cycling_follows_recency_and_reorders_only_when_it_ends() {
        let mut desk = Desk::new();
        let [first, second, third] = [desk.open(), desk.open(), desk.open()];
        let next = action("next-window");
        desk.run(&next);
        assert_eq!(desk.manager.focused(), Some(second));
        desk.run(&next);
        assert_eq!(desk.manager.focused(), Some(first), "still cycling: onward");
        desk.manager.modifiers_released();
        desk.run(&next);
        assert_eq!(
            desk.manager.focused(),
            Some(third),
            "the first is now the most recent, so the next is the one before it"
        );
        assert_eq!(desk.shown().first(), Some(&third), "and it is raised");
    }

    #[test]
    fn a_snap_step_down_minimizes_and_up_restores_while_the_modifiers_are_held() {
        let mut desk = Desk::new();
        let behind = desk.open();
        let window = desk.open();
        desk.run(&action("snap-step down"));
        assert!(desk.window(window).state().is_minimized);
        assert_eq!(desk.manager.focused(), Some(behind));
        desk.run(&action("snap-step up"));
        assert!(!desk.window(window).state().is_minimized);
        assert_eq!(desk.manager.focused(), Some(window));
    }

    #[test]
    fn once_the_modifiers_are_released_a_step_up_acts_on_the_focused_window() {
        let mut desk = Desk::new();
        let behind = desk.open();
        let window = desk.open();
        desk.run(&action("snap-step down"));
        desk.manager.modifiers_released();
        desk.run(&action("snap-step up"));
        assert!(
            desk.window(window).state().is_minimized,
            "it stays minimized"
        );
        assert!(desk.window(behind).placement().is_fully_maximized());
    }

    #[test]
    fn maximizing_fills_the_usable_area_and_toggling_restores_the_floating_area() {
        let mut desk = Desk::new();
        let window = desk.open();
        let floating = desk.outer(window);
        desk.run(&action("maximize"));
        assert_eq!(desk.outer(window), output(0).usable);
        desk.run(&action("maximize"));
        assert_eq!(desk.outer(window), floating);
    }

    #[test]
    fn a_maximized_windows_frame_shows_it_so_and_restoring_shows_it_back() {
        let mut desk = Desk::new();
        let window = desk.open();
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut icons = IconStore::from_environment();
        let style = Style::built_in();
        let mut shows_maximized = |desk: &mut Desk| {
            desk.manager
                .paint_frames(&style, (&mut typesetter, &mut icons));
            desk.window(window).frame().shows_maximized()
        };
        assert!(!shows_maximized(&mut desk), "floating");
        desk.run(&action("maximize"));
        assert!(shows_maximized(&mut desk), "maximized");
        desk.run(&action("maximize"));
        assert!(!shows_maximized(&mut desk), "restored");
    }

    #[test]
    fn a_window_drawing_its_own_frame_gets_none_and_maximizes_its_content() {
        let mut desk = Desk::new();
        let window = desk.open_self_decorated();
        assert!(!desk.window(window).has_frame());
        assert_eq!(
            desk.outer(window),
            desk.window(window).content_area(),
            "nothing around its content"
        );
        desk.run(&action("maximize"));
        assert_eq!(desk.window(window).content_area(), output(0).usable);
    }

    #[test]
    fn a_panel_appearing_makes_maximized_and_tiled_windows_give_way() {
        let mut desk = Desk::new();
        let maximized = desk.open();
        desk.run(&action("maximize"));
        let tiled = desk.open();
        desk.run(&action("tile 500 500 500 500"));
        let floating = desk.open();
        let floating_area = desk.outer(floating);
        desk.manager.set_output_areas(only(30));
        desk.settle();
        let usable = output(30).usable;
        assert_eq!(desk.outer(maximized), usable);
        assert_eq!(
            desk.outer(tiled).loc.y + desk.outer(tiled).size.h,
            usable.size.h,
            "the bottom-right quarter ends where the panel begins"
        );
        assert_eq!(
            desk.outer(floating),
            floating_area,
            "a floating window stays"
        );
    }

    #[test]
    fn a_new_style_keeps_floating_content_still_and_refits_maximized_windows() {
        let mut desk = Desk::new();
        let floating = desk.open();
        let content = desk.window(floating).content_area();
        let maximized = desk.open();
        desk.run(&action("maximize"));
        let taller = Metrics {
            titlebar_height: 40,
            ..METRICS
        };
        desk.manager.restyle(taller);
        desk.settle();
        assert_eq!(desk.window(floating).content_area(), content);
        assert_eq!(
            desk.window(floating).outer_area(taller).size.h,
            content.size.h + 40 + 3 + 1 + 1,
            "the frame grew around it: titlebar, three lines, the handle and its line"
        );
        assert_eq!(desk.window(maximized).outer_area(taller), output(0).usable);
    }

    #[test]
    fn hiding_the_handle_shrinks_floating_frames_and_refits_maximized_windows() {
        let mut desk = Desk::new();
        let floating = desk.open();
        let content = desk.window(floating).content_area();
        let maximized = desk.open();
        desk.run(&action("maximize"));
        let mut settings = desk.config.settings.clone();
        settings.windows.frame.handle = false;
        desk.manager.apply_settings(&settings);
        desk.settle();
        assert_eq!(desk.window(floating).content_area(), content);
        assert_eq!(
            desk.outer(floating).size.h,
            content.size.h + 20 + 3,
            "titlebar and three lines, no handle"
        );
        assert_eq!(desk.outer(maximized), output(0).usable);
        assert_eq!(
            desk.window(maximized).content_area().size.h,
            1080 - 20 - 3,
            "the content takes the handle's room"
        );
    }

    #[test]
    fn sending_a_window_away_focuses_the_one_left_and_showing_it_again_refocuses() {
        let mut desk = Desk::new();
        let staying = desk.open();
        let leaving = desk.open();
        desk.run(&action("send-to-workspace 2"));
        assert_eq!(desk.shown(), [staying]);
        assert_eq!(desk.manager.focused(), Some(staying));
        desk.run(&action("workspace 2"));
        assert_eq!(desk.shown(), [leaving]);
        assert_eq!(desk.manager.focused(), Some(leaving));
    }

    #[test]
    fn taking_a_window_shows_its_new_workspace_with_it_focused() {
        let mut desk = Desk::new();
        desk.open();
        let taken = desk.open();
        desk.run(&action("take-to-workspace 3"));
        assert_eq!(desk.shown(), [taken]);
        assert_eq!(desk.manager.focused(), Some(taken));
    }

    #[test]
    fn removing_the_last_workspace_moves_its_windows_to_the_one_before() {
        let mut desk = Desk::new();
        let window = desk.open();
        desk.run(&action("take-to-workspace 4"));
        desk.run(&action("remove-last-workspace"));
        assert_eq!(
            desk.window(window).workspace(),
            Workspace::numbered(3.try_into().expect("3"))
        );
        assert_eq!(desk.shown(), [window], "and its workspace is shown");
    }

    #[test]
    fn showing_the_desktop_minimizes_every_window_and_again_restores_them() {
        let mut desk = Desk::new();
        let windows = [desk.open(), desk.open()];
        desk.run(&action("show-desktop"));
        assert_eq!(desk.shown(), []);
        assert_eq!(desk.manager.focused(), None);
        desk.run(&action("show-desktop"));
        assert_eq!(desk.shown().len(), windows.len());
    }

    #[test]
    fn a_sticky_window_is_shown_on_every_workspace_until_unstuck() {
        let mut desk = Desk::new();
        let window = desk.open();
        desk.run(&action("stick"));
        desk.run(&action("workspace 2"));
        assert_eq!(desk.shown(), [window]);
        desk.run_on(&action("stick"), Some(window));
        desk.run(&action("workspace 1"));
        assert!(
            desk.shown().is_empty(),
            "unstuck on workspace 2, it stays there"
        );
    }

    #[rstest]
    #[case::close("close", Ending::Close)]
    #[case::kill("kill", Ending::Kill)]
    fn ending_a_window_is_asked_of_its_client(#[case] text: &str, #[case] ending: Ending) {
        let mut desk = Desk::new();
        let window = desk.open();
        desk.run(&action(text));
        assert_eq!(desk.endings, [(window, ending)]);
        assert!(
            desk.manager.window(window).is_some(),
            "until the client closes it"
        );
    }

    #[rstest]
    #[case::maximize(ClientRequest::Maximize, true)]
    #[case::unmaximize_a_floating_window(ClientRequest::Unmaximize, false)]
    #[case::unfullscreen_a_floating_window(ClientRequest::Unfullscreen, false)]
    #[case::minimize(ClientRequest::Minimize, true)]
    fn a_client_request_changes_only_what_it_would_change(
        #[case] request: ClientRequest,
        #[case] is_honored: bool,
    ) {
        let mut desk = Desk::new();
        let window = desk.open();
        assert_eq!(desk.manager.honor(window, request), is_honored);
    }

    #[test]
    fn resizing_to_less_than_the_client_accepts_asks_for_its_minimum() {
        let mut desk = Desk::new();
        let window = desk.manager.add_window(&desk.config.settings, &desk.ledger);
        let committed = CommittedState {
            title: "browser".to_owned(),
            app_id: "app".to_owned(),
            app_id_alternate: String::new(),
            icon: None,
            geometry: Rectangle::from_size((800, 600).into()),
            size_limits: SizeLimits::from_client((500, 83).into(), (0, 0).into()),
            is_self_decorated: false,
        };
        desk.commit(window, committed);
        desk.settle();
        desk.manager.run(&action("resize-to 500 60"), Some(window));
        let asked = desk
            .manager
            .window_mut(window)
            .expect("the window exists")
            .take_request()
            .and_then(|request| request.size);
        assert_eq!(asked, Some(Size::from((500, 83))), "not 500 by 60");
    }

    #[test]
    fn a_program_that_opens_maximized_is_floated_at_a_real_size_not_a_sliver() {
        // Brave asks to be maximized before it has drawn anything, so the
        // window remembers, as the size to return to, a frame round nothing.
        let mut desk = Desk::new();
        let window = desk.manager.add_window(&desk.config.settings, &desk.ledger);
        assert!(desk.manager.honor(window, ClientRequest::Maximize));
        let committed = CommittedState {
            title: "browser".to_owned(),
            app_id: "brave-browser".to_owned(),
            app_id_alternate: String::new(),
            icon: None,
            geometry: Rectangle::from_size((1918, 1000).into()),
            size_limits: SizeLimits::default(),
            is_self_decorated: false,
        };
        desk.commit(window, committed);
        desk.settle();
        assert!(desk.window(window).placement().is_fully_maximized());
        for drag in [
            WindowDrag::Move,
            WindowDrag::Resize {
                edges: ResizeEdges {
                    horizontal: HorizontalEdge::Right,
                    vertical: VerticalEdge::Bottom,
                },
            },
        ] {
            desk.manager.begin_drag(window, drag, (960.0, 40.0).into());
            // A maximized window is floated once the pointer has moved.
            desk.manager.follow_drag(window, (6, 6).into(), 0);
            // The size the client is asked to be: what the drag floats it at.
            let asked = desk
                .manager
                .window_mut(window)
                .expect("the window exists")
                .take_request()
                .and_then(|request| request.size)
                .expect("the client is asked for a size");
            assert!(
                asked.w >= 400 && asked.h >= 300,
                "{drag:?} floats it at a size a window can have: {asked:?}"
            );
            desk.manager.end_drag(window);
            // Back to maximized for the next kind of drag.
            desk.manager.honor(window, ClientRequest::Maximize);
        }
    }

    #[test]
    fn rules_place_a_new_window_where_they_say() {
        let mut config = Config::built_in();
        config.rules = toml::from_str::<Rules>(
            "[[rule]]\nmatch = { app_id = \"steam\" }\nworkspace = 2\nsize = [800, 600]\n\
             position = { anchor = \"center\" }\n",
        )
        .expect("valid rules");
        let mut desk = Desk::with_config(config);
        let other = desk.open();
        let steam = desk.open_app("steam", (300, 200));
        assert_eq!(desk.shown(), [other], "steam opened on workspace 2");
        assert_eq!(
            desk.manager.focused(),
            Some(other),
            "out of sight, it is not focused"
        );
        assert_eq!(
            desk.outer(steam),
            Rectangle::new((560, 240).into(), Size::from((800, 600))),
            "centered at the size the rule sets"
        );
    }

    #[test]
    fn rules_choose_the_output_frame_and_state_and_what_a_window_is_spared() {
        let mut config = Config::built_in();
        config.rules = toml::from_str::<Rules>(
            "[[rule]]\nmatch = { app_id = \"player\" }\noutput = \"RIGHT\"\n\
             frame = \"none\"\nshaded = true\nalpha = 128\nhidden_from_cycling = true\n\
             refuse_fullscreen = true\nhidden_from_window_list = true\n",
        )
        .expect("valid rules");
        let mut desk = Desk::with_config(config);
        let right = OutputArea {
            whole: Rectangle::new((1920, 0).into(), (1920, 1080).into()),
            usable: Rectangle::new((1920, 0).into(), (1920, 1080).into()),
        };
        desk.manager.set_output_areas(
            OutputAreas::new(output(0), vec![right])
                .named(vec!["LEFT".to_owned(), "RIGHT".to_owned()]),
        );
        let other = desk.open();
        let player = desk.open_app("player", (300, 200));
        let window = desk.window(player);
        assert!(
            right.whole.contains(desk.outer(player).loc),
            "on the output named"
        );
        assert!(!window.has_frame());
        assert!(window.state().is_shaded);
        assert!(window.is_exempt(Exemption::WindowList));
        assert!(
            (window.opacity(WindowAlpha {
                focused: 255,
                unfocused: 255
            }) - 128.0 / 255.0)
                .abs()
                < 0.01
        );
        assert!(
            !desk.manager.honor(player, ClientRequest::Fullscreen),
            "its program may not make it fullscreen"
        );
        desk.manager.focus(Some(other));
        desk.run(&action("next-window"));
        assert_eq!(desk.manager.focused(), Some(other), "cycling skips it");
    }

    #[test]
    fn a_moved_window_snaps_to_the_edge_it_comes_near() {
        let mut desk = Desk::new();
        let window = desk.open();
        desk.run(&action("move-to 100 100"));
        assert!(
            desk.manager
                .begin_drag(window, WindowDrag::Move, (150.0, 110.0).into())
        );
        desk.manager.follow_drag(window, (-95, 0).into(), 10);
        assert_eq!(
            desk.outer(window).loc,
            Point::from((0, 100)),
            "5 pixels away: snapped"
        );
        desk.manager.follow_drag(window, (-50, 0).into(), 10);
        assert_eq!(
            desk.outer(window).loc,
            Point::from((50, 100)),
            "out of reach"
        );
        desk.manager.end_drag(window);
    }

    /// A desk whose windows snap to the edges, the layout picker on.
    fn snapping_desk() -> Desk {
        let mut config = Config::built_in();
        config.settings.windows.snapping.enabled = true;
        config.settings.windows.snapping.layout_picker = true;
        Desk::with_config(config)
    }

    #[test]
    fn a_window_let_go_at_the_left_edge_takes_the_left_half_and_restores_back() {
        let mut desk = snapping_desk();
        let window = desk.open();
        desk.run(&action("move-to 100 100"));
        let before = desk.outer(window);
        assert!(
            desk.manager
                .begin_drag(window, WindowDrag::Move, (150.0, 110.0).into())
        );
        desk.manager.follow_drag(window, (-145, 300).into(), 0);
        desk.manager.follow_drag_pointer(window, (5, 410).into());
        let usable = output(0).usable;
        let left_half = Rectangle::new(usable.loc, (usable.size.w / 2, usable.size.h).into());
        assert_eq!(desk.manager.snap_preview(), Some((window, left_half)));
        desk.manager.end_drag(window);
        desk.settle();
        assert_eq!(desk.outer(window), left_half);
        assert_eq!(desk.manager.snap_preview(), None, "the preview goes");
        desk.run(&action("maximize"));
        desk.run(&action("maximize"));
        assert_eq!(desk.outer(window), before, "back where the move began");
    }

    #[test]
    fn the_top_edge_maximizes_and_the_picker_puts_the_window_on_a_cell() {
        let mut desk = snapping_desk();
        let window = desk.open();
        assert!(
            desk.manager
                .begin_drag(window, WindowDrag::Move, (150.0, 110.0).into())
        );
        desk.manager.follow_drag_pointer(window, (960, 2).into());
        assert_eq!(
            desk.manager.snap_preview().map(|(_, area)| area),
            Some(output(0).usable),
            "the top edge maximizes"
        );
        let shown = desk.manager.layout_picker().expect("the picker shows");
        let thumb = shown.picker.thumbs()[1];
        let right_cell = thumb.loc + Point::from((thumb.size.w * 3 / 4, thumb.size.h / 2));
        desk.manager.follow_drag_pointer(window, right_cell);
        let usable = output(0).usable;
        let right_half = Rectangle::new(
            (usable.loc.x + usable.size.w / 2, usable.loc.y).into(),
            (usable.size.w / 2, usable.size.h).into(),
        );
        assert_eq!(
            desk.manager.snap_preview().map(|(_, area)| area),
            Some(right_half),
            "the even split's right cell"
        );
        desk.manager.end_drag(window);
        desk.settle();
        assert_eq!(desk.outer(window), right_half);
    }

    #[test]
    fn a_window_placed_while_being_moved_stays_where_it_was_placed() {
        let mut desk = Desk::new();
        let window = desk.open();
        assert!(
            desk.manager
                .begin_drag(window, WindowDrag::Move, (50.0, 10.0).into())
        );
        desk.run(&action("maximize"));
        desk.manager.follow_drag(window, (200, 150).into(), 0);
        assert_eq!(
            desk.outer(window),
            output(0).usable,
            "the drag no longer moves it"
        );
    }

    #[test]
    fn resizing_from_the_top_left_keeps_the_bottom_right_corner_still() {
        let mut desk = Desk::new();
        let window = desk.open();
        desk.run(&action("move-to 300 300"));
        let before = desk.window(window).content_area();
        let edges = ResizeEdges {
            horizontal: HorizontalEdge::Left,
            vertical: VerticalEdge::Top,
        };
        desk.manager
            .begin_drag(window, WindowDrag::Resize { edges }, (300.0, 300.0).into());
        desk.manager.follow_drag(window, (-40, -30).into(), 0);
        desk.settle();
        desk.manager.end_drag(window);
        desk.settle();
        let after = desk.window(window).content_area();
        assert_eq!(after.size, before.size + Size::from((40, 30)));
        assert_eq!(
            after.loc + after.size.to_point(),
            before.loc + before.size.to_point()
        );
    }

    #[test]
    fn a_resize_from_the_left_stays_anchored_when_the_client_commits_the_old_size_first() {
        // An X11 program commits again at its old size for changes of its
        // own (its title, say) before it takes the new size: the window must
        // not take that commit for the answer and stop holding its right edge.
        let mut desk = Desk::new();
        let window = desk.open();
        desk.run(&action("move-to 300 300"));
        let before = desk.window(window).content_area();
        let edges = ResizeEdges {
            horizontal: HorizontalEdge::Left,
            vertical: VerticalEdge::Bottom,
        };
        desk.manager
            .begin_drag(window, WindowDrag::Resize { edges }, (300.0, 300.0).into());
        desk.manager.follow_drag(window, (-40, 30).into(), 0);
        desk.manager.end_drag(window);
        let committing = |desk: &mut Desk, size: Size<i32, Logical>| {
            let mut committed = desk.committed[&window].clone();
            committed.geometry = Rectangle::from_size(size);
            desk.commit(window, committed);
        };
        committing(&mut desk, before.size);
        committing(&mut desk, before.size + Size::from((40, 30)));
        let after = desk.window(window).content_area();
        assert_eq!(after.size, before.size + Size::from((40, 30)));
        assert_eq!(
            after.loc.x + after.size.w,
            before.loc.x + before.size.w,
            "the right edge did not move: the window grew to the left"
        );
        assert_eq!(after.loc.y, before.loc.y, "the top did not move");
    }

    #[test]
    fn changes_between_settles_reach_the_client_as_one_request() {
        let mut desk = Desk::new();
        let window = desk.open();
        desk.manager.run(&action("maximize"), Some(window));
        desk.manager.run(&action("shade"), Some(window));
        desk.manager.run(&action("maximize vertical"), Some(window));
        let updates = desk.manager.take_client_updates();
        assert_eq!(updates.states.len(), 1);
        let (_, requested) = updates.states[0];
        assert_eq!(
            requested.placement,
            PlacementKind::Maximized(Axes::Horizontal)
        );
        assert!(requested.is_activated);
    }

    #[test]
    fn a_window_out_of_sight_cannot_take_focus() {
        let mut desk = Desk::new();
        let shown = desk.open();
        let hidden = desk.open();
        desk.run(&action("minimize"));
        desk.manager.focus(Some(hidden));
        assert_eq!(desk.manager.focused(), Some(shown));
    }

    /// One thing that can happen to a desk.
    #[derive(Debug, Clone)]
    enum Event {
        /// A client opens a window with content of this size.
        Open(i32, i32),
        /// The client of the window at this position among the windows
        /// closes it.
        Close(usize),
        /// A binding runs an action on the window at this position, or the
        /// focused one.
        Run(WindowManagerAction, Option<usize>),
        /// The client of the window at this position asks something.
        Ask(usize, ClientRequest),
        /// The user lets go of the modifiers.
        ReleaseModifiers,
        /// A panel this tall appears at the bottom, or goes for 0.
        Panel(i32),
        /// The user drags the window at this position by this much.
        Drag(usize, WindowDrag, i32, i32),
    }

    /// The window-management actions a random desk runs.
    fn actions() -> Vec<WindowManagerAction> {
        [
            "close",
            "raise",
            "lower",
            "focus",
            "maximize",
            "maximize horizontal",
            "fullscreen",
            "minimize",
            "shade",
            "stick",
            "snap-step left",
            "snap-step right",
            "snap-step up",
            "snap-step down",
            "tile 0 0 500 1000",
            "move 30 -20",
            "move-to 50 60",
            "resize 25 -40",
            "resize-to 640 480",
            "workspace next",
            "workspace last",
            "workspace 3",
            "send-to-workspace 2",
            "take-to-workspace previous",
            "add-workspace",
            "remove-last-workspace",
            "next-window",
            "previous-window",
            "show-desktop",
        ]
        .into_iter()
        .map(action)
        .collect()
    }

    /// Any event.
    fn event() -> impl Strategy<Value = Event> {
        let edges = ResizeEdges {
            horizontal: HorizontalEdge::Right,
            vertical: VerticalEdge::Bottom,
        };
        prop_oneof![
            3 => (50..1500_i32, 50..900_i32).prop_map(|(width, height)| Event::Open(width, height)),
            1 => any::<usize>().prop_map(Event::Close),
            8 => (proptest::sample::select(actions()), proptest::option::of(any::<usize>()))
                .prop_map(|(action, window)| Event::Run(action, window)),
            2 => (any::<usize>(), proptest::sample::select(vec![
                ClientRequest::Maximize,
                ClientRequest::Unmaximize,
                ClientRequest::Fullscreen,
                ClientRequest::Unfullscreen,
                ClientRequest::Minimize,
            ]))
                .prop_map(|(window, request)| Event::Ask(window, request)),
            2 => Just(Event::ReleaseModifiers),
            1 => prop_oneof![Just(0), Just(30), Just(200)].prop_map(Event::Panel),
            2 => (
                any::<usize>(),
                proptest::sample::select(vec![WindowDrag::Move, WindowDrag::Resize { edges }]),
                -300..300_i32,
                -300..300_i32,
            )
                .prop_map(|(window, drag, x, y)| Event::Drag(window, drag, x, y)),
        ]
    }

    impl Desk {
        /// Returns the window at `position` among the windows, top first.
        fn nth(&self, position: usize) -> Option<WindowId> {
            let windows: Vec<WindowId> = self.manager.windows_top_down().map(Window::id).collect();
            (!windows.is_empty()).then(|| windows[position % windows.len()])
        }

        /// Lets `event` happen.
        fn apply(&mut self, event: &Event) {
            match event {
                Event::Open(width, height) => {
                    self.open_app("app", (*width, *height));
                }
                Event::Close(position) => {
                    if let Some(window) = self.nth(*position) {
                        self.close(window);
                    }
                }
                Event::Run(action, position) => {
                    let window =
                        position.map_or(self.manager.focused(), |position| self.nth(position));
                    self.run_on(action, window);
                }
                Event::Ask(position, request) => {
                    if let Some(window) = self.nth(*position) {
                        self.manager.honor(window, *request);
                    }
                }
                Event::ReleaseModifiers => self.manager.modifiers_released(),
                Event::Panel(height) => self.manager.set_output_areas(only(*height)),
                Event::Drag(position, drag, x, y) => {
                    if let Some(window) = self.nth(*position) {
                        let pointer = self.outer(window).loc.to_f64() + Point::from((5.0, 5.0));
                        self.manager.begin_drag(window, *drag, pointer);
                        self.manager.follow_drag(window, (*x, *y).into(), 10);
                        self.settle();
                        self.manager.end_drag(window);
                    }
                }
            }
            self.settle();
        }

        /// Panics unless every invariant of the window manager holds.
        fn check(&self) {
            let manager = &self.manager;
            let current = manager.workspaces.current();
            if let Some(focused) = manager.focused() {
                let window = manager.window(focused).expect("the focused window exists");
                assert!(window.is_shown_on(current), "the focused window is shown");
            }
            assert_eq!(
                manager.windows_top_down().count(),
                manager.window_count(),
                "every window is stacked once"
            );
            assert!(manager.workspaces.contains(current));
            let usable = manager.outputs.primary().usable;
            for window in manager.windows_top_down() {
                assert!(
                    manager.workspaces.contains(window.workspace()),
                    "its workspace exists"
                );
                if !window.is_placed() {
                    continue;
                }
                match window.placement() {
                    Placement::Maximized {
                        axes: Axes::Both, ..
                    }
                    | Placement::Tiled { .. } => {
                        let outer = window.outer_area(manager.metrics());
                        assert!(
                            usable.contains_rect(outer),
                            "{outer:?} lies within the usable {usable:?}"
                        );
                    }
                    Placement::Fullscreen { .. } => {
                        assert_eq!(window.content_area(), manager.outputs.primary().whole);
                    }
                    Placement::Floating | Placement::Maximized { .. } => {}
                }
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn every_invariant_holds_whatever_happens(
            events in proptest::collection::vec(event(), 1..60),
        ) {
            let mut desk = Desk::new();
            for event in &events {
                desk.apply(event);
                desk.check();
            }
        }
    }
}

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

mod focus;
mod placement_strategy;
mod window;
mod windows;
mod workspaces;

use smithay::utils::{Logical, Point, Rectangle};
use vaht_ui::theme::Metrics;
use vaht_ui::{Theme, Typesetter};

pub(crate) use self::focus::CycleDirection;
use self::focus::Focus;
pub(crate) use self::placement_strategy::{ColumnDirection, PlacementStrategy, Room, RowDirection};
pub(crate) use self::window::AlphaChange;
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
    Config, CycleOrder, FocusStealing, FrameSettings, Settings, WindowCycle, WindowManagerAction,
};
use crate::resources::ResourceLedger;

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
    /// Where windows can be placed.
    output: OutputArea,
    /// The theme's frame sizes.
    theme_metrics: Metrics,
    /// What frames show, from the settings.
    frame: FrameSettings,
    /// The frame sizes windows are laid out by: the theme's, as `frame`
    /// takes them.
    metrics: Metrics,
    /// How a resize by hand is shown, from the settings.
    resize_style: ResizeStyle,
    /// How opaque windows are unless `set-alpha` says, from the settings.
    default_alpha: WindowAlpha,
    /// Windows asked to end, until the compositor takes them.
    endings: Vec<(WindowId, Ending)>,
    /// Whether anything shown changed since the compositor last asked.
    is_redraw_needed: bool,
}

impl WindowManager {
    /// Creates a window manager with no windows, its workspaces and frames
    /// as `settings` say, placing windows on `output` with frames sized by
    /// the theme's `metrics`.
    pub(crate) fn new(settings: &Settings, output: OutputArea, metrics: Metrics) -> Self {
        let frame = settings.windows.frame.clone();
        Self {
            windows: Windows::default(),
            workspaces: Workspaces::named(settings.workspaces.names()),
            focus: Focus::default(),
            snap_minimized: None,
            held_titlebar: None,
            output,
            theme_metrics: metrics,
            metrics: frame.frame_metrics(metrics),
            frame,
            resize_style: Self::resize_style(settings),
            default_alpha: settings.windows.alpha(),
            endings: Vec::new(),
            is_redraw_needed: false,
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

    /// Returns the frame sizes windows are laid out by: the theme's, less
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
        let frame = Frame::new(settings.windows.frame.titlebar_layout(), ledger);
        let id = self
            .windows
            .insert(frame, self.workspaces.current(), ledger);
        tracing::debug!(window = %id, "window opened");
        if settings.focus.focus_new_windows {
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
        self.endings.retain(|(ending, _)| *ending != id);
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
                tracing::debug!(window = %focused.id(), title = focused.title(), "focused");
            }
        }
        self.focus.set(window);
    }

    /// Brings `window` into view and gives it focus: restores it if it is
    /// minimized, shows its workspace, and raises it.
    pub(crate) fn activate(&mut self, window: WindowId) {
        let Some(target) = self.window_mut(window) else {
            return;
        };
        target.set_minimized(false);
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

    /// Moves `window` above all others.
    pub(crate) fn raise(&mut self, window: WindowId) {
        self.is_redraw_needed |= self.windows.raise(window);
    }

    /// Moves `window` below all others.
    pub(crate) fn lower(&mut self, window: WindowId) {
        self.is_redraw_needed |= self.windows.lower(window);
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
        }
    }

    /// Honors what a client asks for `window`, if it changes anything;
    /// returns whether it did. A client asked nothing new must still hear
    /// back, so the compositor answers it when this returns false.
    pub(crate) fn honor(&mut self, window: WindowId, request: ClientRequest) -> bool {
        let Some(placement) = self.windows.get(window).map(Window::placement) else {
            return false;
        };
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

    /// Ends the drag of `window`.
    pub(crate) fn end_drag(&mut self, window: WindowId) {
        if let Some(target) = self.window_mut(window) {
            target.end_drag();
            tracing::debug!(%window, "drag ended");
        }
        self.is_redraw_needed = true;
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

    /// Lets go of a held titlebar button with the pointer at `location`,
    /// running its action if the pointer is still on it.
    pub(crate) fn release_titlebar_button(&mut self, location: Point<f64, Logical>) {
        let Some(window) = self.held_titlebar.take() else {
            return;
        };
        let metrics = self.metrics;
        let clicked = self
            .window_mut(window)
            .and_then(|target| target.release_titlebar_button(location, metrics));
        if let Some(button) = clicked {
            let action = button.action();
            tracing::debug!(%action, %window, "titlebar button clicked");
            self.run(&action, Some(window));
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
        if settings.windows.frame != self.frame {
            self.frame = settings.windows.frame.clone();
            self.restyle(self.theme_metrics);
        }
        self.focus_most_recent();
        self.is_redraw_needed = true;
    }

    /// Places windows on `output` from now on; maximized, tiled and
    /// fullscreen windows follow it at once, as when a panel appears.
    pub(crate) fn set_output_area(&mut self, output: OutputArea) {
        if output == self.output {
            return;
        }
        self.output = output;
        self.refit_placed_windows();
    }

    /// Lays windows out with frames sized by `metrics` from now on, and
    /// paints every frame again: the theme changed. A floating window's
    /// content stays where it is; the others are placed again.
    pub(crate) fn restyle(&mut self, metrics: Metrics) {
        self.theme_metrics = metrics;
        self.metrics = self.frame.frame_metrics(metrics);
        for window in self.windows.iter_mut() {
            window.invalidate_frame();
        }
        self.refit_placed_windows();
    }

    /// Paints every window's frame that changed.
    pub(crate) fn paint_frames(&mut self, theme: &Theme, typesetter: &mut Typesetter) {
        let metrics = self.metrics;
        for window in self.windows.iter_mut() {
            window.paint_frame(metrics, theme, typesetter);
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

    /// Places maximized, tiled and fullscreen windows again, in the output's
    /// areas and with frames sized as they are now.
    fn refit_placed_windows(&mut self) {
        let (output, metrics) = (self.output, self.metrics);
        for window in self.windows.iter_mut() {
            let placement = window.placement().clone();
            if window.is_placed() && placement != Placement::Floating {
                window.place(placement, output, metrics);
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
    /// settings and rules, among the windows already shown.
    fn place_new_window(&mut self, id: WindowId, config: &Config, pointer: Point<i32, Logical>) {
        let (metrics, output, current) = (self.metrics, self.output, self.workspaces.current());
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
        let rule = config.rules.for_window(window, current);
        let size = rule.size().unwrap_or(window.outer_area(metrics).size);
        let corner = rule.position.map_or_else(
            || config.settings.placement.place(size, &room),
            |position| position.corner(room.usable, size),
        );
        let outer = Rectangle::new(corner, size);
        let workspace = rule
            .workspace
            .map(Workspace::numbered)
            .filter(|workspace| self.workspaces.contains(*workspace));
        let Some(window) = self.window_mut(id) else {
            return;
        };
        window.place_initially(outer, metrics);
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
        if let Some(layer) = rule.layer {
            self.windows.set_layer(id, layer);
        }
        if let Some(focus_stealing) = rule.focus_stealing
            && let Some(window) = self.window_mut(id)
        {
            window.set_focus_stealing(focus_stealing);
        }
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

    /// Applies `change` to `window`, given the theme's metrics.
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
        let output = self.output;
        self.change_window(window, |target, metrics| {
            let current = target.outer_area(metrics);
            let placement = change(target.placement().clone(), current);
            target.place(placement, output, metrics);
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
        let (metrics, usable) = (self.metrics, self.output.usable);
        let Some(target) = window.and_then(|id| self.windows.get(id)) else {
            return;
        };
        let current = target.outer_area(metrics);
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
        self.change_window(window, |target, _| target.set_minimized(true));
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
    /// pixels: the usable area's, and the other shown windows'.
    fn edge_snap_for(&self, window: WindowId, distance: u16) -> EdgeSnap {
        let others = self
            .shown_top_down()
            .filter(|other| other.id() != window)
            .map(|other| other.outer_area(self.metrics))
            .collect();
        EdgeSnap::new(distance, self.output.usable, others)
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
        menu: vaht_ui::theme::MenuMetrics {
            title_height: 20,
            item_height: 20,
            border_width: 1,
            corners: vaht_ui::CornerRadii::SQUARE,
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
                manager: WindowManager::new(&config.settings, output(0), METRICS),
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
                icon: None,
                geometry: Rectangle::from_size(size.into()),
                size_limits: SizeLimits::default(),
            };
            self.commit(window, committed);
            self.settle();
            window
        }

        /// Opens a window of 400 × 300 content.
        fn open(&mut self) -> WindowId {
            self.open_app("app", (400, 300))
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
    fn a_panel_appearing_makes_maximized_and_tiled_windows_give_way() {
        let mut desk = Desk::new();
        let maximized = desk.open();
        desk.run(&action("maximize"));
        let tiled = desk.open();
        desk.run(&action("tile 500 500 500 500"));
        let floating = desk.open();
        let floating_area = desk.outer(floating);
        desk.manager.set_output_area(output(30));
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
    fn a_new_theme_keeps_floating_content_still_and_refits_maximized_windows() {
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
                Event::Panel(height) => self.manager.set_output_area(output(*height)),
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
            let usable = manager.output.usable;
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
                        assert_eq!(window.content_area(), manager.output.whole);
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

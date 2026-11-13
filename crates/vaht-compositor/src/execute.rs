//! Running commands: [`Compositor::execute`] is the one dispatcher (01 §8).
//!
//! Key bindings, mouse bindings, and later menus and IPC, all hand it a
//! parsed [`Command`]; nothing dispatches on strings. Each action's behavior
//! lives with the subsystem it changes, so the match below only delegates.

use smithay::input::pointer::GrabStartData;
use smithay::utils::{SERIAL_COUNTER, Serial};

use crate::Compositor;
use vaht_ui::theme::Metrics;

use crate::config::{Action, Command, ResizeFrom, WindowMatcher};
use crate::grab::GrabKind;
use crate::window::{Area, Direction, Placement, SnapStep, Window, WindowDrag, WindowId};
use crate::workspaces::WorkspaceTarget;

/// The mouse button press a command runs for.
#[derive(Debug, Clone)]
pub(crate) struct Press {
    /// Where, and with which button.
    pub(crate) start: GrabStartData<Compositor>,
    /// The press's serial.
    pub(crate) serial: Serial,
}

/// What a command acts on.
#[derive(Debug)]
pub(crate) struct ExecutionContext<'grab> {
    /// The window: the one clicked for a mouse binding, the focused one for
    /// a key binding.
    pub(crate) window: Option<WindowId>,
    /// The button press, for a mouse binding; `start-moving` and
    /// `start-resizing` need one.
    pub(crate) press: Option<Press>,
    /// The pointer grab a drag binding runs in: `start-moving` and
    /// `start-resizing` turn it into a move or resize instead of starting a
    /// new grab, which cannot be done from inside one.
    pub(crate) grab: Option<&'grab mut GrabKind>,
}

impl ExecutionContext<'_> {
    /// A context with only a window, as for a key binding.
    pub(crate) const fn for_window(window: Option<WindowId>) -> Self {
        Self {
            window,
            press: None,
            grab: None,
        }
    }

    /// A context for a mouse button press on `window`, or the desktop.
    pub(crate) const fn for_press(window: Option<WindowId>, press: Press) -> Self {
        Self {
            window,
            press: Some(press),
            grab: None,
        }
    }
}

impl Compositor {
    /// Runs `command` in `context`.
    pub(crate) fn execute(&mut self, command: &Command, context: &mut ExecutionContext<'_>) {
        match command {
            Command::Run(action) => self.run_action(action, context),
            Command::Sequence(commands) => {
                for command in commands {
                    self.execute(command, context);
                }
            }
            // The key handling repeats it; here it runs once.
            Command::Repeat { command, .. } => self.execute(command, context),
        }
    }

    /// Runs one action.
    fn run_action(&mut self, action: &Action, context: &mut ExecutionContext<'_>) {
        tracing::debug!(%action, window = ?context.window, "running an action");
        match action {
            Action::Exec(command_line) => self.launcher.run(command_line),
            Action::Close => {
                if let Some(window) = context.window.and_then(|id| self.windows.get(id)) {
                    window.toplevel().close();
                }
            }
            Action::Kill => self.kill_program_of(context.window),
            Action::Raise => {
                if let Some(window) = context.window {
                    self.raise_window(window);
                }
            }
            Action::Lower => {
                if let Some(window) = context.window {
                    self.lower_window(window);
                }
            }
            Action::Focus => {
                if context.window.is_some() {
                    self.focus_window(context.window, SERIAL_COUNTER.next_serial());
                }
            }
            Action::StartMoving => self.start_drag(None, context),
            Action::StartResizing(from) => self.start_drag(Some(*from), context),
            Action::Reload => self.reload_configuration(),
            Action::Exit => self.stop(),
            Action::SetEnvironment { name, value } => {
                self.launcher.set_environment(name.clone(), value.clone());
            }
            Action::Maximize(axes) => self
                .change_placement(context.window, |placement, current| {
                    placement.with_maximized_toggled(*axes, current)
                }),
            Action::Fullscreen => {
                self.change_placement(context.window, Placement::with_fullscreen_toggled);
            }
            Action::SnapStep(direction) => self.snap_step(context.window, *direction),
            Action::Tile(cell) => {
                self.change_placement(context.window, |placement, current| Placement::Tiled {
                    cell: *cell,
                    restore: placement.restore_area(current),
                });
            }
            Action::Minimize => self.set_minimized(context.window, true),
            Action::Shade => self.change_window(context.window, |window, _| {
                window.set_shaded(!window.state().is_shaded);
            }),
            Action::Stick => self.toggle_sticky(context.window),
            Action::Move(distance) => self.change_window(context.window, |window, metrics| {
                window.move_by(*distance, metrics);
            }),
            Action::MoveTo(position) => self.change_window(context.window, |window, metrics| {
                let corner = window.outer_area(metrics).loc;
                window.move_by(*position - corner, metrics);
            }),
            Action::Resize(growth) => self.change_window(context.window, |window, metrics| {
                let size = window.content_area().size;
                let grown = ((size.w + growth.x).max(1), (size.h + growth.y).max(1));
                window.resize_to(grown.into(), metrics);
            }),
            Action::ResizeTo(size) => self.change_window(context.window, |window, metrics| {
                window.resize_to(*size, metrics);
            }),
            Action::Workspace(target) => self.show_workspace(*target),
            Action::SendToWorkspace(target) => {
                self.send_to_workspace(context.window, *target, false);
            }
            Action::TakeToWorkspace(target) => {
                self.send_to_workspace(context.window, *target, true);
            }
            Action::AddWorkspace => {
                let added = self.workspaces.add();
                tracing::debug!(workspace = %added, "workspace added");
            }
            Action::RemoveLastWorkspace => self.remove_last_workspace(),
            Action::NextWindow(windows) => self.cycle_focus(windows, true),
            Action::PreviousWindow(windows) => self.cycle_focus(windows, false),
            Action::ShowDesktop => self.show_desktop(),
        }
    }

    /// Applies `change` to `window`, given the theme's metrics, and redraws.
    fn change_window(
        &mut self,
        window: Option<WindowId>,
        change: impl FnOnce(&mut Window, Metrics),
    ) {
        let metrics = self.theme.metrics();
        if let Some(target) = window.and_then(|id| self.windows.get_mut(id)) {
            change(target, metrics);
            self.schedule_render();
        }
    }

    /// Places `window` where `change` says, given its placement and the
    /// area it covers now.
    pub(crate) fn change_placement(
        &mut self,
        window: Option<WindowId>,
        change: impl FnOnce(Placement, Area) -> Placement,
    ) {
        let output = self.output_area();
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
                self.activate_window(minimized);
            }
            return;
        }
        let output = self.output_area();
        let metrics = self.theme.metrics();
        let Some(target) = window.and_then(|id| self.windows.get(id)) else {
            return;
        };
        let current = target.outer_area(metrics);
        match target
            .placement()
            .clone()
            .snap_step(direction, current, output.usable)
        {
            SnapStep::Place(placement) => self.change_placement(window, |_, _| placement),
            SnapStep::Minimize => {
                self.set_minimized(window, true);
                self.snap_minimized = window;
            }
            SnapStep::Unchanged => {}
        }
    }

    /// Minimizes `window`, or restores it.
    pub(crate) fn set_minimized(&mut self, window: Option<WindowId>, is_minimized: bool) {
        self.change_window(window, |target, _| target.set_minimized(is_minimized));
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

    /// Shows the workspace `target` means.
    fn show_workspace(&mut self, target: WorkspaceTarget) {
        let Some(workspace) = self.workspaces.resolve(target) else {
            return;
        };
        if self.workspaces.show(workspace) {
            tracing::debug!(%workspace, "workspace shown");
            self.focus_most_recent();
            self.schedule_render();
        }
    }

    /// Puts `window` on the workspace `target` means, and with `follow`
    /// shows that workspace too.
    fn send_to_workspace(
        &mut self,
        window: Option<WindowId>,
        target: WorkspaceTarget,
        follow: bool,
    ) {
        let Some(workspace) = self.workspaces.resolve(target) else {
            return;
        };
        self.change_window(window, |target, _| target.move_to_workspace(workspace));
        if follow && self.workspaces.show(workspace) {
            tracing::debug!(%workspace, "workspace shown");
        }
        self.focus_most_recent();
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
        self.schedule_render();
    }

    /// Focuses and raises the next window (or the previous one) among those
    /// `windows` matches, in the order windows last had focus. Minimized
    /// windows count only if the matcher asks for them.
    fn cycle_focus(&mut self, windows: &WindowMatcher, forward: bool) {
        let current = self.workspaces.current();
        let candidates = self
            .windows
            .top_down()
            .filter(|window| {
                (windows.admits_minimized() || !window.state().is_minimized)
                    && windows.matches(window, current)
            })
            .map(Window::id)
            .collect::<Vec<_>>();
        let ordered = self.focus.by_recency(candidates);
        if let Some(next) = self.focus.step(ordered, forward) {
            self.activate_window(next);
        }
    }

    /// Brings `window` into view and gives it focus: restores it if it is
    /// minimized, shows its workspace, and raises it.
    pub(crate) fn activate_window(&mut self, window: WindowId) {
        let Some(target) = self.windows.get_mut(window) else {
            return;
        };
        target.set_minimized(false);
        let (workspace, is_sticky) = (target.workspace(), target.state().is_sticky);
        if !is_sticky && self.workspaces.show(workspace) {
            tracing::debug!(%workspace, "workspace shown");
        }
        self.raise_window(window);
        self.focus_window(Some(window), SERIAL_COUNTER.next_serial());
    }

    /// Minimizes every window shown, or, when none is, restores the
    /// minimized windows of the current workspace.
    fn show_desktop(&mut self) {
        let current = self.workspaces.current();
        let is_desktop_shown = self.windows.shown_top_down(current).next().is_none();
        for window in self.windows.iter_mut() {
            let is_here = window.state().is_sticky || window.workspace() == current;
            if is_here {
                window.set_minimized(!is_desktop_shown);
            }
        }
        self.focus_most_recent();
        self.schedule_render();
    }

    /// Starts moving the context's window with the pointer, or resizing it
    /// when `resize` says from where, or turns the drag in progress into
    /// that.
    fn start_drag(&mut self, resize: Option<ResizeFrom>, context: &mut ExecutionContext<'_>) {
        let (Some(window), Some(press)) = (context.window, context.press.as_ref()) else {
            tracing::warn!(
                "start-moving and start-resizing work only in mouse bindings on a window"
            );
            return;
        };
        let metrics = self.theme.metrics();
        let Some(target) = self.windows.get(window) else {
            return;
        };
        let drag = match resize {
            None => WindowDrag::Move,
            Some(from) => WindowDrag::Resize {
                edges: from.edges(target.frame_geometry(metrics).outer, press.start.location),
            },
        };
        if let Some(grab) = context.grab.as_deref_mut() {
            self.turn_grab_into(window, grab, drag, press.start.location);
        } else {
            let (start, serial) = (press.start.clone(), press.serial);
            self.start_window_grab(window, drag, start, serial);
        }
    }

    /// Ends the program that owns `window`, as Fluxbox's `Kill` does.
    fn kill_program_of(&self, window: Option<WindowId>) {
        if let Some(window) = window.and_then(|id| self.windows.get(id)) {
            window.toplevel().kill_program(&self.display_handle);
        }
    }
}

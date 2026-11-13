//! Running commands: [`Compositor::execute`] is the one dispatcher (01 §8).
//!
//! Key bindings, mouse bindings, and later menus and IPC, all hand it a
//! parsed [`Command`]; nothing dispatches on strings. Each action's behavior
//! lives with the subsystem it changes, so the match below only delegates.

use smithay::input::pointer::GrabStartData;
use smithay::reexports::wayland_server::Resource as _;
use smithay::utils::{SERIAL_COUNTER, Serial};

use crate::Compositor;
use crate::config::{Action, Command, ResizeFrom};
use crate::grab::GrabKind;
use crate::window::{ResizeEdges, WindowId};

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
        }
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
        let kind = match resize {
            None => GrabKind::Move {
                initial_location: target.location(),
            },
            Some(ResizeFrom::NearestCorner) => GrabKind::Resize {
                edges: ResizeEdges::nearest_corner(
                    target.frame_geometry(metrics).outer,
                    press.start.location,
                ),
            },
            Some(ResizeFrom::Edges(edges)) => GrabKind::Resize { edges },
        };
        if let Some(grab) = context.grab.as_deref_mut() {
            self.turn_grab_into(window, grab, kind);
        } else {
            let (start, serial) = (press.start.clone(), press.serial);
            self.start_window_grab(window, kind, start, serial);
        }
    }

    /// Ends the program that owns `window`, as Fluxbox's `Kill` does.
    fn kill_program_of(&self, window: Option<WindowId>) {
        let Some(window) = window.and_then(|id| self.windows.get(id)) else {
            return;
        };
        let surface = window.toplevel().wl_surface();
        let Ok(client) = self.display_handle.get_client(surface.id()) else {
            return;
        };
        let pid = match client.get_credentials(&self.display_handle) {
            Ok(credentials) => credentials.pid,
            Err(error) => {
                tracing::warn!(%error, "could not learn which program owns the window");
                return;
            }
        };
        if u32::try_from(pid).is_ok_and(|pid| pid == std::process::id()) || pid <= 0 {
            tracing::warn!(pid, "not killing: the window is vaht's own");
            return;
        }
        tracing::info!(pid, "killing a window's program");
        let signal = nix::sys::signal::Signal::SIGKILL;
        if let Err(error) = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal) {
            tracing::warn!(%error, pid, "could not kill a window's program");
        }
    }
}

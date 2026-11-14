//! The windows as taskbars and window switchers outside vaht see and work
//! them (`zwlr_foreign_toplevel_manager_v1`): waybar's taskbar, rofi's
//! window list, and the like.
//!
//! Each program binding the manager gets a handle for each window, told
//! its title, program, state (maximized, minimized, activated,
//! fullscreen) and outputs; vaht tells it again whatever changes
//! ([`WindowFacts::changes`], plain values that tests follow alone), and
//! that the window closed. What a program asks of a handle is the window
//! manager's to do ([`HandleRequest`]).

use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1};
use smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};

use crate::Compositor;
use crate::client::ClientState;
use crate::window_manager::WindowId;

/// How a window covers its output, as the protocol tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Covering {
    /// As it was put, or tiled.
    #[default]
    Placed,
    /// Filling the usable area.
    Maximized,
    /// Covering its output.
    Fullscreen,
}

/// What a window's state is, as the protocol tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ToplevelStates {
    /// How it covers its output.
    pub(crate) covering: Covering,
    /// Hidden until restored.
    pub(crate) is_minimized: bool,
    /// Having keyboard focus.
    pub(crate) is_activated: bool,
}

impl ToplevelStates {
    /// Returns the states as the protocol's array carries them: each one
    /// that holds, as a 32-bit number in native byte order.
    fn as_array(self) -> Vec<u8> {
        use zwlr_foreign_toplevel_handle_v1::State;
        [
            (self.covering == Covering::Maximized, State::Maximized),
            (self.is_minimized, State::Minimized),
            (self.is_activated, State::Activated),
            (self.covering == Covering::Fullscreen, State::Fullscreen),
        ]
        .into_iter()
        .filter(|(holds, _)| *holds)
        .flat_map(|(_, state)| u32::from(state).to_ne_bytes())
        .collect()
    }
}

/// What programs are told of a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowFacts<Place> {
    /// Its title.
    pub(crate) title: String,
    /// Its program.
    pub(crate) app_id: String,
    /// Its state.
    pub(crate) states: ToplevelStates,
    /// The outputs it is on.
    pub(crate) outputs: Vec<Place>,
}

/// One thing to tell programs of a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FactChange<Place> {
    /// Its title is this.
    Title(String),
    /// Its program is this.
    AppId(String),
    /// Its state is this.
    States(ToplevelStates),
    /// It is now on this output.
    OutputEnter(Place),
    /// It is no longer on this output.
    OutputLeave(Place),
}

impl<Place: Clone + PartialEq> WindowFacts<Place> {
    /// Returns what to tell programs that knew `known` (nothing, for a
    /// window new to them) for them to know these facts.
    pub(crate) fn changes(&self, known: Option<&Self>) -> Vec<FactChange<Place>> {
        let mut changes = Vec::new();
        if known.is_none_or(|known| known.title != self.title) {
            changes.push(FactChange::Title(self.title.clone()));
        }
        if known.is_none_or(|known| known.app_id != self.app_id) {
            changes.push(FactChange::AppId(self.app_id.clone()));
        }
        if known.is_none_or(|known| known.states != self.states) {
            changes.push(FactChange::States(self.states));
        }
        let was_on: &[Place] = known.map_or(&[], |known| &known.outputs);
        changes.extend(
            was_on
                .iter()
                .filter(|output| !self.outputs.contains(output))
                .cloned()
                .map(FactChange::OutputLeave),
        );
        changes.extend(
            self.outputs
                .iter()
                .filter(|output| !was_on.contains(output))
                .cloned()
                .map(FactChange::OutputEnter),
        );
        changes
    }
}

/// What a program asks of a window's handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HandleRequest {
    /// Maximize it, or not.
    Maximize(bool),
    /// Minimize it, or restore it.
    Minimize(bool),
    /// Bring it forward with focus.
    Activate,
    /// Ask it to close.
    Close,
    /// Make it cover an output, or not.
    Fullscreen(bool),
}

/// A window as told to programs.
#[derive(Debug)]
struct Told {
    /// The window.
    window: WindowId,
    /// What programs were told of it.
    facts: WindowFacts<Output>,
    /// Its handle in each program's manager.
    handles: Vec<ZwlrForeignToplevelHandleV1>,
}

/// The managers bound, and the windows they were told of.
#[derive(Debug)]
pub(crate) struct ToplevelManagement {
    /// The global, held to keep it offered.
    _global: GlobalId,
    /// Each manager bound and not stopped.
    managers: Vec<ZwlrForeignToplevelManagerV1>,
    /// Each window told of, in the order they opened.
    told: Vec<Told>,
}

impl ToplevelManagement {
    /// The protocol version vaht serves.
    pub(crate) const VERSION: u32 = 3;

    /// Offers the global on `display`, to programs not in a sandbox.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let global =
            display.create_global::<Compositor, ZwlrForeignToplevelManagerV1, _>(Self::VERSION, ());
        Self {
            _global: global,
            managers: Vec::new(),
            told: Vec::new(),
        }
    }

    /// Returns whether `client` may bind the manager.
    pub(crate) fn may_bind(client: &smithay::reexports::wayland_server::Client) -> bool {
        ClientState::is_unrestricted(client)
    }

    /// Follows `manager`, just bound: it is told of every window.
    pub(crate) fn bind(&mut self, manager: ZwlrForeignToplevelManagerV1, display: &DisplayHandle) {
        for told in &mut self.told {
            if let Some(handle) = Self::announce(&manager, told.window, &told.facts, display) {
                told.handles.push(handle);
            }
        }
        self.managers.push(manager);
    }

    /// Forgets `manager`, which stopped: it is told of no more windows.
    pub(crate) fn stop(&mut self, manager: &ZwlrForeignToplevelManagerV1) {
        self.forget(manager);
        manager.finished();
    }

    /// Forgets `manager`, which is gone.
    pub(crate) fn forget(&mut self, manager: &ZwlrForeignToplevelManagerV1) {
        self.managers.retain(|bound| bound != manager);
    }

    /// Follows the windows now open, `windows`, each with its facts, in
    /// the order they opened: new ones are announced to every manager,
    /// changed ones told again, and gone ones closed.
    pub(crate) fn follow(
        &mut self,
        windows: Vec<(WindowId, WindowFacts<Output>)>,
        display: &DisplayHandle,
    ) {
        let (kept, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.told)
            .into_iter()
            .partition(|told| windows.iter().any(|(window, _)| *window == told.window));
        for handle in gone.iter().flat_map(|told| &told.handles) {
            handle.closed();
        }
        self.told = kept;
        for (window, facts) in windows {
            if let Some(told) = self.told.iter_mut().find(|told| told.window == window) {
                let changes = facts.changes(Some(&told.facts));
                if !changes.is_empty() {
                    told.handles.retain(Resource::is_alive);
                    for handle in &told.handles {
                        Self::tell(handle, &changes);
                    }
                    told.facts = facts;
                }
            } else {
                let handles = self
                    .managers
                    .iter()
                    .filter_map(|manager| Self::announce(manager, window, &facts, display))
                    .collect();
                self.told.push(Told {
                    window,
                    facts,
                    handles,
                });
            }
        }
    }

    /// Makes `window`'s handle for `manager`'s program and tells it
    /// `facts`; none if the program is gone.
    fn announce(
        manager: &ZwlrForeignToplevelManagerV1,
        window: WindowId,
        facts: &WindowFacts<Output>,
        display: &DisplayHandle,
    ) -> Option<ZwlrForeignToplevelHandleV1> {
        let client = manager.client()?;
        let handle = client
            .create_resource::<ZwlrForeignToplevelHandleV1, WindowId, Compositor>(
                display,
                manager.version(),
                window,
            )
            .ok()?;
        manager.toplevel(&handle);
        Self::tell(&handle, &facts.changes(None));
        Some(handle)
    }

    /// Tells `handle`'s program of `changes`, then that they are done.
    fn tell(handle: &ZwlrForeignToplevelHandleV1, changes: &[FactChange<Output>]) {
        let client = handle.client();
        for change in changes {
            match change {
                FactChange::Title(title) => handle.title(title.clone()),
                FactChange::AppId(app_id) => handle.app_id(app_id.clone()),
                FactChange::States(states) => handle.state(states.as_array()),
                FactChange::OutputEnter(output) | FactChange::OutputLeave(output) => {
                    let Some(client) = &client else { continue };
                    for wl_output in output.client_outputs(client) {
                        if matches!(change, FactChange::OutputEnter(_)) {
                            handle.output_enter(&wl_output);
                        } else {
                            handle.output_leave(&wl_output);
                        }
                    }
                }
            }
        }
        handle.done();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Facts of a window titled `title` on `outputs`.
    fn facts(title: &str, outputs: &[u8]) -> WindowFacts<u8> {
        WindowFacts {
            title: title.to_owned(),
            app_id: "term".to_owned(),
            states: ToplevelStates::default(),
            outputs: outputs.to_vec(),
        }
    }

    #[test]
    fn a_new_window_is_told_everything() {
        assert_eq!(
            facts("Shell", &[1]).changes(None),
            [
                FactChange::Title("Shell".to_owned()),
                FactChange::AppId("term".to_owned()),
                FactChange::States(ToplevelStates::default()),
                FactChange::OutputEnter(1),
            ]
        );
    }

    #[test]
    fn a_known_window_is_told_only_what_changed() {
        let known = facts("Shell", &[1]);
        assert_eq!(known.changes(Some(&known)), []);
        let mut now = facts("Shell — ~", &[2]);
        now.states.is_activated = true;
        assert_eq!(
            now.changes(Some(&known)),
            [
                FactChange::Title("Shell — ~".to_owned()),
                FactChange::States(now.states),
                FactChange::OutputLeave(1),
                FactChange::OutputEnter(2),
            ]
        );
    }

    #[test]
    fn states_are_sent_as_the_protocol_numbers_them() {
        let states = ToplevelStates {
            covering: Covering::Maximized,
            is_activated: true,
            ..ToplevelStates::default()
        };
        let numbers: Vec<u32> = states
            .as_array()
            .chunks(4)
            .map(|bytes| u32::from_ne_bytes(bytes.try_into().expect("4 bytes")))
            .collect();
        assert_eq!(numbers, [0, 2], "maximized and activated");
    }
}

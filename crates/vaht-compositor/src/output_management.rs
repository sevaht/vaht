//! Outputs as programs outside vaht see and arrange them
//! (`zwlr_output_manager_v1`): wlr-randr, kanshi and wdisplays; fluxland
//! served this through wlroots.
//!
//! Each program binding the manager gets a head per connected output, with
//! its modes, and is told each head's state (on or off, mode, position,
//! transform, scale) whenever it changes, then `done` with a new serial.
//! [`HeadState::of`] reads an output's state; what changed is decided from
//! plain values, so tests follow it alone. A configuration a program
//! applies is carried out by the compositor as `[[output]]` entries laid
//! out exactly ([`HeadRequest`]); it lasts until `outputs.toml` is read
//! again or monitors come or go, when the configuration applies again.

use std::sync::{Arc, Mutex};

use smithay::output::{Mode, Output};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_head_v1::{self, ZwlrOutputHeadV1};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_manager_v1::ZwlrOutputManagerV1;
use smithay::reexports::wayland_protocols_wlr::output_management::v1::server::zwlr_output_mode_v1::ZwlrOutputModeV1;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use smithay::utils::{Logical, Point, Transform};

use crate::Compositor;

/// What a head tells of its output.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HeadState {
    /// Whether the output is on.
    pub(crate) is_enabled: bool,
    /// Its mode, while on.
    pub(crate) mode: Option<Mode>,
    /// Where its corner is in the layout.
    pub(crate) position: Point<i32, Logical>,
    /// How it is turned.
    pub(crate) transform: Transform,
    /// How much larger everything is drawn on it.
    pub(crate) scale: f64,
}

impl HeadState {
    /// Returns `output`'s state, on (`is_enabled`) or off.
    pub(crate) fn of(output: &Output, is_enabled: bool) -> Self {
        Self {
            is_enabled,
            mode: output.current_mode().filter(|_| is_enabled),
            position: output.current_location(),
            transform: output.current_transform(),
            scale: output.current_scale().fractional_scale(),
        }
    }
}

/// What a configuration asks of one output, while it is being made.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct HeadRequest {
    /// The mode: its size and, unless any will do, refresh in millihertz.
    pub(crate) mode: Option<(i32, i32, Option<i32>)>,
    /// Where its corner goes.
    pub(crate) position: Option<Point<i32, Logical>>,
    /// How it is turned.
    pub(crate) transform: Option<Transform>,
    /// How much larger everything is drawn on it.
    pub(crate) scale: Option<f64>,
}

/// A configuration being made: each output it names, and what it asks of
/// each one it turns on (none for one it turns off).
#[derive(Debug, Default)]
pub(crate) struct ConfigurationState {
    /// The serial of the heads it was made for.
    pub(crate) serial: u32,
    /// Each output named.
    pub(crate) heads: Vec<(Output, Option<Arc<Mutex<HeadRequest>>>)>,
    /// Whether it was applied or tested: it may be used only once.
    pub(crate) is_used: bool,
}

/// The user data of a configuration.
pub(crate) type Configuration = Mutex<ConfigurationState>;

/// A head sent to a manager, with its modes.
#[derive(Debug)]
struct SentHead {
    /// The output.
    output: Output,
    /// The head.
    head: ZwlrOutputHeadV1,
    /// Each mode sent, with its object.
    modes: Vec<(Mode, ZwlrOutputModeV1)>,
}

/// A manager bound, and the heads sent to it.
#[derive(Debug)]
struct Manager {
    /// The manager.
    manager: ZwlrOutputManagerV1,
    /// Each head sent.
    heads: Vec<SentHead>,
}

/// The managers bound, and the outputs' state last told.
#[derive(Debug)]
pub(crate) struct OutputManagement {
    /// The global, held to keep it offered.
    _global: GlobalId,
    /// The serial of the state last told.
    serial: u32,
    /// Each manager bound and not stopped.
    managers: Vec<Manager>,
    /// Each connected output's state, as last told.
    told: Vec<(Output, HeadState)>,
}

impl OutputManagement {
    /// The protocol version vaht serves.
    pub(crate) const VERSION: u32 = 4;

    /// Offers the global on `display`.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let global = display.create_global::<Compositor, ZwlrOutputManagerV1, _>(Self::VERSION, ());
        Self {
            _global: global,
            serial: 1,
            managers: Vec::new(),
            told: Vec::new(),
        }
    }

    /// Returns the serial of the state last told.
    pub(crate) const fn serial(&self) -> u32 {
        self.serial
    }

    /// Follows `manager`, just bound: it is sent a head for each output.
    pub(crate) fn bind(&mut self, manager: ZwlrOutputManagerV1, display: &DisplayHandle) {
        let mut bound = Manager {
            manager,
            heads: Vec::new(),
        };
        for (output, state) in &self.told {
            Self::announce(&mut bound, output, state, display);
        }
        bound.manager.done(self.serial);
        self.managers.push(bound);
    }

    /// Forgets `manager`, which stopped or is gone; it is told it is
    /// finished if `is_stopped`.
    pub(crate) fn forget(&mut self, manager: &ZwlrOutputManagerV1, is_stopped: bool) {
        self.managers.retain(|bound| bound.manager != *manager);
        if is_stopped {
            manager.finished();
        }
    }

    /// Follows the connected outputs, each on or off, as `connected` says:
    /// if anything changed, each manager is told and given a new serial.
    pub(crate) fn follow(&mut self, connected: Vec<(Output, HeadState)>, display: &DisplayHandle) {
        if connected == self.told {
            return;
        }
        self.serial = self.serial.wrapping_add(1);
        for bound in &mut self.managers {
            bound.heads.retain(|sent| {
                let is_kept = connected.iter().any(|(output, _)| *output == sent.output);
                if !is_kept {
                    for (_, mode) in &sent.modes {
                        mode.finished();
                    }
                    sent.head.finished();
                }
                is_kept
            });
            for (output, state) in &connected {
                let known = self.told.iter().find(|(told, _)| told == output);
                match known {
                    None => Self::announce(bound, output, state, display),
                    Some((_, before)) if before != state => {
                        if let Some(sent) = bound.heads.iter().find(|sent| sent.output == *output) {
                            Self::tell_state(sent, state);
                        }
                    }
                    Some(_) => {}
                }
            }
            bound.manager.done(self.serial);
        }
        self.told = connected;
    }

    /// Sends `bound` a head for `output`, with its modes and `state`.
    fn announce(bound: &mut Manager, output: &Output, state: &HeadState, display: &DisplayHandle) {
        let Some(client) = bound.manager.client() else {
            return;
        };
        let version = bound.manager.version();
        let Ok(head) = client.create_resource::<ZwlrOutputHeadV1, Output, Compositor>(
            display,
            version,
            output.clone(),
        ) else {
            return;
        };
        bound.manager.head(&head);
        head.name(output.name());
        head.description(output.description());
        let physical = output.physical_properties();
        if physical.size.w > 0 && physical.size.h > 0 {
            head.physical_size(physical.size.w, physical.size.h);
        }
        let preferred = output.preferred_mode();
        let mut modes = Vec::new();
        for mode in output.modes() {
            let Ok(object) = client
                .create_resource::<ZwlrOutputModeV1, Mode, Compositor>(display, version, mode)
            else {
                continue;
            };
            head.mode(&object);
            object.size(mode.size.w, mode.size.h);
            object.refresh(mode.refresh);
            if preferred == Some(mode) {
                object.preferred();
            }
            modes.push((mode, object));
        }
        if version >= zwlr_output_head_v1::EVT_MAKE_SINCE {
            head.make(physical.make.clone());
            head.model(physical.model.clone());
            head.serial_number(physical.serial_number.clone());
        }
        let sent = SentHead {
            output: output.clone(),
            head,
            modes,
        };
        Self::tell_state(&sent, state);
        bound.heads.push(sent);
    }

    /// Tells `sent`'s client its output's `state`.
    fn tell_state(sent: &SentHead, state: &HeadState) {
        let head = &sent.head;
        head.enabled(i32::from(state.is_enabled));
        if !state.is_enabled {
            return;
        }
        if let Some((_, object)) = state
            .mode
            .and_then(|mode| sent.modes.iter().find(|(known, _)| *known == mode))
        {
            head.current_mode(object);
        }
        head.position(state.position.x, state.position.y);
        head.transform(state.transform.into());
        head.scale(state.scale);
    }
}

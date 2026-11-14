//! Turning monitors off and on without changing the layout
//! (`zwlr_output_power_manager_v1`), as `wlopm` and swayidle's
//! `output * power off` do; fluxland did this through wlroots.
//!
//! An output turned off keeps its place and its windows; its monitor
//! sleeps (on the DRM backend) until it is turned on again or the user
//! does anything at all, so a screen is never left dark with no way back.
//! Every program controlling an output's power is told its mode whenever
//! it changes, and that the control failed once the output is gone.

use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1;
use smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_v1::{Mode, ZwlrOutputPowerV1};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::GlobalId;

use crate::Compositor;

/// Which outputs are off, and the programs controlling their power.
#[derive(Debug)]
pub(crate) struct OutputPower {
    /// The global, held to keep it offered.
    _global: GlobalId,
    /// Each control and the output it is for.
    controls: Vec<(ZwlrOutputPowerV1, Output)>,
    /// The outputs turned off.
    off: Vec<Output>,
}

impl OutputPower {
    /// The protocol version vaht serves.
    pub(crate) const VERSION: u32 = 1;

    /// Offers the global on `display`.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let global =
            display.create_global::<Compositor, ZwlrOutputPowerManagerV1, _>(Self::VERSION, ());
        Self {
            _global: global,
            controls: Vec::new(),
            off: Vec::new(),
        }
    }

    /// Returns whether `output` is on.
    pub(crate) fn is_on(&self, output: &Output) -> bool {
        !self.off.contains(output)
    }

    /// Follows `control`, just made for `output`, telling it the mode.
    pub(crate) fn add_control(&mut self, control: ZwlrOutputPowerV1, output: Output) {
        control.mode(self.mode_of(&output));
        self.controls.push((control, output));
    }

    /// Forgets `control`, which is gone.
    pub(crate) fn forget_control(&mut self, control: &ZwlrOutputPowerV1) {
        self.controls.retain(|(known, _)| known != control);
    }

    /// Turns `output` on or off (`is_on`); returns whether that changed
    /// anything. Its controls are told.
    pub(crate) fn set(&mut self, output: &Output, is_on: bool) -> bool {
        if self.is_on(output) == is_on {
            return false;
        }
        if is_on {
            self.off.retain(|off| off != output);
        } else {
            self.off.push(output.clone());
        }
        let mode = self.mode_of(output);
        for (control, _) in self.controls.iter().filter(|(_, of)| of == output) {
            control.mode(mode);
        }
        true
    }

    /// Turns every output that is off on again; returns them.
    pub(crate) fn wake_all(&mut self) -> Vec<Output> {
        let woken = self.off.clone();
        for output in &woken {
            self.set(output, true);
        }
        woken
    }

    /// Follows `output` going away: its controls fail.
    pub(crate) fn output_gone(&mut self, output: &Output) {
        self.off.retain(|off| off != output);
        self.controls.retain(|(control, of)| {
            let is_kept = of != output;
            if !is_kept {
                control.failed();
            }
            is_kept
        });
    }

    /// Returns the mode `output` is in.
    fn mode_of(&self, output: &Output) -> Mode {
        if self.is_on(output) {
            Mode::On
        } else {
            Mode::Off
        }
    }
}

#[cfg(test)]
mod tests {
    use smithay::output::{PhysicalProperties, Subpixel};

    use super::*;

    /// An output named `name`.
    fn output(name: &str) -> Output {
        Output::new(
            name.to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "vaht".to_owned(),
                model: "test".to_owned(),
                serial_number: String::new(),
            },
        )
    }

    #[test]
    fn an_output_turned_off_stays_off_until_turned_on_or_woken() {
        let display =
            smithay::reexports::wayland_server::Display::<Compositor>::new().expect("a display");
        let mut power = OutputPower::new(&display.handle());
        let (left, right) = (output("LEFT"), output("RIGHT"));
        assert!(power.set(&left, false));
        assert!(!power.set(&left, false), "already off");
        assert!(!power.is_on(&left));
        assert!(power.is_on(&right));
        assert!(power.set(&right, false));
        assert!(power.set(&right, true));
        assert_eq!(
            power.wake_all(),
            std::slice::from_ref(&left),
            "only the one still off"
        );
        assert!(power.is_on(&left));
        assert_eq!(power.wake_all(), Vec::<Output>::new());
        power.set(&left, false);
        power.output_gone(&left);
        assert!(power.is_on(&left), "forgotten once gone");
    }
}

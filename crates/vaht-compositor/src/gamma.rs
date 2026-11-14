//! Night-light tools changing an output's colors
//! (`zwlr_gamma_control_manager_v1`), as wlsunset and gammastep do;
//! fluxland did this through wlroots.
//!
//! One program at a time controls an output's gamma: it is told the ramp
//! size, and sends a red, green and blue ramp of that size. When its
//! control goes, the output's colors go back to normal. [`GammaRamps`]
//! reads ramps from what a program sends; [`GammaControls`] keeps which
//! program controls which output.

use std::io::{self, Read};

use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1;
use smithay::reexports::wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_v1::ZwlrGammaControlV1;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::GlobalId;

use crate::Compositor;

/// A ramp per color channel: the value each input level shows as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GammaRamps {
    /// Red's.
    pub(crate) red: Vec<u16>,
    /// Green's.
    pub(crate) green: Vec<u16>,
    /// Blue's.
    pub(crate) blue: Vec<u16>,
}

impl GammaRamps {
    /// Reads three ramps of `size` values each from `source`: red, green,
    /// then blue, each value 16 bits in native byte order, as the protocol
    /// lays them out.
    ///
    /// # Errors
    ///
    /// Returns an error if `source` holds too little.
    pub(crate) fn read(mut source: impl Read, size: u32) -> io::Result<Self> {
        let size = usize::try_from(size).map_err(io::Error::other)?;
        let mut bytes = vec![0; size * 3 * 2];
        source.read_exact(&mut bytes)?;
        let values: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_ne_bytes(*pair))
            .collect();
        let (red, rest) = values.split_at(size);
        let (green, blue) = rest.split_at(size);
        Ok(Self {
            red: red.to_vec(),
            green: green.to_vec(),
            blue: blue.to_vec(),
        })
    }

    /// Returns ramps of `size` values that change nothing: each level
    /// shows as itself.
    pub(crate) fn identity(size: u32) -> Self {
        let steps = size.saturating_sub(1).max(1);
        let ramp: Vec<u16> = (0..size)
            .map(|level| {
                let value = u64::from(level) * u64::from(u16::MAX) / u64::from(steps);
                u16::try_from(value).unwrap_or(u16::MAX)
            })
            .collect();
        Self {
            red: ramp.clone(),
            green: ramp.clone(),
            blue: ramp,
        }
    }
}

/// Which program controls each output's gamma.
#[derive(Debug)]
pub(crate) struct GammaControls {
    /// The global, held to keep it offered.
    _global: GlobalId,
    /// Each control, the output it is for, and the ramp size told.
    controls: Vec<(ZwlrGammaControlV1, Output, u32)>,
}

impl GammaControls {
    /// The protocol version vaht serves.
    pub(crate) const VERSION: u32 = 1;

    /// Offers the global on `display`.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let global =
            display.create_global::<Compositor, ZwlrGammaControlManagerV1, _>(Self::VERSION, ());
        Self {
            _global: global,
            controls: Vec::new(),
        }
    }

    /// Follows `control`, just made for `output`, whose ramps have `size`
    /// values (none where the gamma cannot change): it controls the output
    /// unless another does, and fails otherwise.
    pub(crate) fn add(&mut self, control: ZwlrGammaControlV1, output: Output, size: Option<u32>) {
        let is_taken = self.controls.iter().any(|(_, of, _)| *of == output);
        match size {
            Some(size) if !is_taken && size > 0 => {
                control.gamma_size(size);
                self.controls.push((control, output, size));
            }
            _ => control.failed(),
        }
    }

    /// Returns the output `control` controls and its ramp size, if it
    /// controls one.
    pub(crate) fn output_of(&self, control: &ZwlrGammaControlV1) -> Option<(Output, u32)> {
        self.controls
            .iter()
            .find(|(known, ..)| known == control)
            .map(|(_, output, size)| (output.clone(), *size))
    }

    /// Ends `control` after it sent something unusable: it fails, and
    /// controls nothing any more; returns the output it controlled.
    pub(crate) fn fail(&mut self, control: &ZwlrGammaControlV1) -> Option<Output> {
        control.failed();
        self.forget(control)
    }

    /// Forgets `control`, which is gone; returns the output it controlled,
    /// whose colors go back to normal.
    pub(crate) fn forget(&mut self, control: &ZwlrGammaControlV1) -> Option<Output> {
        let index = self
            .controls
            .iter()
            .position(|(known, ..)| known == control)?;
        Some(self.controls.remove(index).1)
    }

    /// Follows `output` going away: its control fails.
    pub(crate) fn output_gone(&mut self, output: &Output) {
        self.controls.retain(|(control, of, _)| {
            let is_kept = of != output;
            if !is_kept {
                control.failed();
            }
            is_kept
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramps_are_read_red_green_then_blue() {
        let values: [u16; 6] = [1, 2, 3, 4, 5, 6];
        let bytes: Vec<u8> = values
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect();
        let ramps = GammaRamps::read(bytes.as_slice(), 2).expect("enough");
        assert_eq!(ramps.red, [1, 2]);
        assert_eq!(ramps.green, [3, 4]);
        assert_eq!(ramps.blue, [5, 6]);
        assert!(
            GammaRamps::read(&bytes[..10], 2).is_err(),
            "too little is refused"
        );
    }

    #[test]
    fn the_identity_runs_from_black_to_white() {
        let ramps = GammaRamps::identity(3);
        assert_eq!(ramps.red, [0, 32767, u16::MAX]);
        assert_eq!(ramps.red, ramps.blue);
        assert_eq!(GammaRamps::identity(1).green, [0]);
    }
}

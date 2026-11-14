//! The DRM backend's decisions, made from plain values (R-TEST8).
//!
//! What the kernel reports goes in as data (a connector's modes, a GPU
//! driver's name, a key) and what to do comes out as data; the driver in
//! the rest of the backend only reads the devices and carries the answers
//! out. Every case is tested here, without hardware.

use smithay::input::keyboard::Keysym;

use crate::config::{OutputMode, Vrr};

/// One mode a connector offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConnectorMode {
    /// Its width, in pixels.
    pub(crate) width: i32,
    /// Its height, in pixels.
    pub(crate) height: i32,
    /// Its refresh rate, in millihertz.
    pub(crate) refresh_millihertz: i32,
    /// Whether the monitor prefers it: its native resolution.
    pub(crate) is_preferred: bool,
}

/// How well the mode chosen matches the one asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModeFit {
    /// No mode was asked for: the monitor's preferred one.
    Preferred,
    /// The size and refresh rate asked for (the refresh rate within half a
    /// hertz), or the size asked for at its fastest refresh rate.
    Exact,
    /// The size asked for, at the refresh rate nearest the one asked for.
    NearestRefresh,
    /// The monitor has no mode of the size asked for: its preferred one.
    Unavailable,
}

/// Where an output's frames are, between vaht drawing them and the
/// monitor showing them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FrameState {
    /// Nothing to draw, nothing on its way.
    #[default]
    Idle,
    /// A frame should be drawn as soon as vaht gets to it.
    Due,
    /// A frame is on its way to the monitor, which shows it at its next
    /// vertical blank; `is_again_due` says another was asked for since.
    Queued {
        /// Whether a frame was asked for while this one was on its way.
        is_again_due: bool,
    },
    /// The last frame changed nothing, so nothing went to the monitor;
    /// vaht waits a refresh interval before drawing again, so a client
    /// committing unchanged frames does not make vaht spin.
    Throttled {
        /// Whether a frame was asked for while waiting.
        is_again_due: bool,
    },
}

/// What the window on top of an output is, as variable refresh asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TopWindow {
    /// A fullscreen window: a game or a video.
    Fullscreen,
    /// Any other window, or none.
    Other,
}

/// What one GPU driver needs done differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GpuQuirks {
    /// Whether windows may be shown on overlay planes, sparing the GPU
    /// from compositing them. NVIDIA's driver shows them wrongly.
    pub(crate) allows_overlay_planes: bool,
}

/// Picks the mode of `modes` that `wanted` asks for, or the preferred one
/// without it; none if the connector offers no mode at all.
pub(crate) fn choose_mode(
    modes: &[ConnectorMode],
    wanted: Option<OutputMode>,
) -> Option<(usize, ModeFit)> {
    let preferred = modes
        .iter()
        .position(|mode| mode.is_preferred)
        .or_else(|| (!modes.is_empty()).then_some(0))?;
    let Some(wanted) = wanted else {
        return Some((preferred, ModeFit::Preferred));
    };
    let same_size: Vec<(usize, &ConnectorMode)> = modes
        .iter()
        .enumerate()
        .filter(|(_, mode)| mode.width == wanted.width && mode.height == wanted.height)
        .collect();
    let Some(&(fastest, _)) = same_size
        .iter()
        .max_by_key(|(_, mode)| mode.refresh_millihertz)
    else {
        return Some((preferred, ModeFit::Unavailable));
    };
    let Some(refresh) = wanted.refresh_millihertz else {
        return Some((fastest, ModeFit::Exact));
    };
    let (nearest, distance) = same_size
        .iter()
        .map(|(index, mode)| (*index, mode.refresh_millihertz.abs_diff(refresh)))
        .min_by_key(|(_, distance)| *distance)?;
    let fit = if distance <= 500 {
        ModeFit::Exact
    } else {
        ModeFit::NearestRefresh
    };
    Some((nearest, fit))
}

/// Returns whether an output's refresh rate should follow what is shown,
/// by the `vrr` setting, when it `is_supported`, with `top` the window on
/// top of it.
pub(crate) const fn wants_vrr(vrr: Vrr, is_supported: bool, top: TopWindow) -> bool {
    is_supported
        && match vrr {
            Vrr::Off => false,
            Vrr::On => true,
            Vrr::Fullscreen => matches!(top, TopWindow::Fullscreen),
        }
}

/// Returns the virtual terminal `keysym` switches to (Ctrl+Alt+F1 gives
/// `XF86Switch_VT_1`), if it is one of those keys.
pub(crate) fn virtual_terminal(keysym: Keysym) -> Option<i32> {
    let first = Keysym::XF86_Switch_VT_1.raw();
    let last = Keysym::XF86_Switch_VT_12.raw();
    let raw = keysym.raw();
    (first..=last)
        .contains(&raw)
        .then(|| i32::try_from(raw - first + 1).ok())
        .flatten()
}

impl FrameState {
    /// Asks for a frame; returns whether the output just became due, so a
    /// draw must be arranged. While a frame is on its way, the next waits
    /// for it.
    pub(crate) const fn request(&mut self) -> bool {
        match self {
            Self::Idle => {
                *self = Self::Due;
                true
            }
            Self::Due => false,
            Self::Queued { is_again_due } | Self::Throttled { is_again_due } => {
                *is_again_due = true;
                false
            }
        }
    }

    /// Returns whether a frame should be drawn now.
    pub(crate) const fn is_due(self) -> bool {
        matches!(self, Self::Due)
    }

    /// Notes that a frame was drawn: `is_queued` if it went to the monitor,
    /// else it changed nothing and vaht waits a refresh interval.
    pub(crate) const fn drawn(&mut self, is_queued: bool) {
        *self = if is_queued {
            Self::Queued {
                is_again_due: false,
            }
        } else {
            Self::Throttled {
                is_again_due: false,
            }
        };
    }

    /// Notes that the frame on its way was shown (a vertical blank), or the
    /// wait after one that changed nothing is over; returns whether a
    /// frame is due again.
    pub(crate) const fn finished(&mut self) -> bool {
        let is_due = matches!(
            self,
            Self::Queued { is_again_due: true } | Self::Throttled { is_again_due: true }
        );
        *self = if is_due { Self::Due } else { Self::Idle };
        is_due
    }

    /// Forgets any frame on its way: the session was paused, and resuming
    /// draws everything anew.
    pub(crate) const fn reset(&mut self) {
        *self = Self::Idle;
    }
}

impl GpuQuirks {
    /// The quirks of the driver called `name`, described by
    /// `description`, as the kernel gives them.
    pub(crate) fn for_driver(name: &str, description: &str) -> Self {
        let is_nvidia = [name, description]
            .iter()
            .any(|text| text.to_ascii_lowercase().contains("nvidia"));
        Self {
            allows_overlay_planes: !is_nvidia,
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A monitor's modes: 1920×1080 at 60 (preferred), 144 and 59.94 Hz,
    /// and 1280×720 at 60 Hz.
    const MODES: [ConnectorMode; 4] = [
        mode(1920, 1080, 60_000, true),
        mode(1920, 1080, 144_000, false),
        mode(1920, 1080, 59_940, false),
        mode(1280, 720, 60_000, false),
    ];

    /// A mode.
    const fn mode(
        width: i32,
        height: i32,
        refresh_millihertz: i32,
        is_preferred: bool,
    ) -> ConnectorMode {
        ConnectorMode {
            width,
            height,
            refresh_millihertz,
            is_preferred,
        }
    }

    /// A mode asked for.
    const fn wanted(width: i32, height: i32, refresh_millihertz: Option<i32>) -> OutputMode {
        OutputMode {
            width,
            height,
            refresh_millihertz,
        }
    }

    #[rstest]
    #[case::nothing_asked(None, (0, ModeFit::Preferred))]
    #[case::size_alone_takes_the_fastest(Some(wanted(1920, 1080, None)), (1, ModeFit::Exact))]
    #[case::size_and_rate(Some(wanted(1920, 1080, Some(144_000))), (1, ModeFit::Exact))]
    #[case::a_rate_written_roughly(Some(wanted(1920, 1080, Some(59_900))), (2, ModeFit::Exact))]
    #[case::a_rate_it_lacks(Some(wanted(1920, 1080, Some(120_000))), (1, ModeFit::NearestRefresh))]
    #[case::a_size_it_lacks(Some(wanted(2560, 1440, None)), (0, ModeFit::Unavailable))]
    #[case::a_smaller_size(Some(wanted(1280, 720, Some(60_000))), (3, ModeFit::Exact))]
    fn the_mode_asked_for_is_found(
        #[case] asked: Option<OutputMode>,
        #[case] expected: (usize, ModeFit),
    ) {
        assert_eq!(choose_mode(&MODES, asked), Some(expected));
    }

    #[test]
    fn a_connector_without_modes_has_no_mode_to_choose() {
        assert_eq!(choose_mode(&[], None), None);
        let without_preference = [mode(800, 600, 60_000, false)];
        assert_eq!(
            choose_mode(&without_preference, None),
            Some((0, ModeFit::Preferred))
        );
    }

    #[test]
    fn a_frame_asked_for_while_one_is_on_its_way_follows_it() {
        let mut state = FrameState::default();
        assert!(state.request(), "an idle output becomes due");
        assert!(!state.request(), "asking again changes nothing");
        assert!(state.is_due());
        state.drawn(true);
        assert!(!state.request(), "waits for the frame on its way");
        assert!(state.finished(), "and is due again once it is shown");
        state.drawn(true);
        assert!(!state.finished(), "with nothing asked, it rests");
        assert_eq!(state, FrameState::Idle);
    }

    #[test]
    fn a_frame_that_changed_nothing_waits_a_refresh_before_the_next() {
        let mut state = FrameState::default();
        let _ = state.request();
        state.drawn(false);
        assert!(!state.request(), "not drawn again at once");
        assert!(state.finished(), "but after the wait");
        state.reset();
        assert!(state.request(), "after a pause it starts over");
    }

    #[rstest]
    #[case::nvidias_name("nvidia-drm", "NVIDIA DRM driver", false)]
    #[case::amd("amdgpu", "AMD GPU", true)]
    #[case::intel("i915", "Intel Graphics", true)]
    fn overlay_planes_are_used_except_on_nvidia(
        #[case] name: &str,
        #[case] description: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(
            GpuQuirks::for_driver(name, description).allows_overlay_planes,
            expected
        );
    }

    #[rstest]
    #[case::off(Vrr::Off, true, TopWindow::Fullscreen, false)]
    #[case::on(Vrr::On, true, TopWindow::Other, true)]
    #[case::fullscreen_with_one(Vrr::Fullscreen, true, TopWindow::Fullscreen, true)]
    #[case::fullscreen_without(Vrr::Fullscreen, true, TopWindow::Other, false)]
    #[case::unsupported(Vrr::On, false, TopWindow::Fullscreen, false)]
    fn vrr_follows_the_setting(
        #[case] vrr: Vrr,
        #[case] is_supported: bool,
        #[case] top: TopWindow,
        #[case] expected: bool,
    ) {
        assert_eq!(wants_vrr(vrr, is_supported, top), expected);
    }

    #[test]
    fn the_terminal_switch_keys_name_their_terminal() {
        assert_eq!(virtual_terminal(Keysym::XF86_Switch_VT_1), Some(1));
        assert_eq!(virtual_terminal(Keysym::XF86_Switch_VT_12), Some(12));
        assert_eq!(virtual_terminal(Keysym::F1), None);
    }
}

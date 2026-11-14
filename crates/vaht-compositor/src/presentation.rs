//! Telling clients when their frames showed (`wp_presentation`), which
//! video players and games pace themselves by.
//!
//! After an output's frame is drawn, the scene hands over the presentation
//! feedback of the surfaces in it
//! ([`crate::scene::Scene::presentation_feedback`]). A monitor reports the
//! moment it showed the frame (the page flip), with its own count of
//! refreshes; outputs with no such moment (headless, and a window in a
//! host compositor) report at once, counting their own frames
//! ([`FrameCounter`]).

use std::time::Duration;

use smithay::backend::drm::DrmEventMetadata;
use smithay::backend::drm::DrmEventTime;
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind;
use smithay::utils::{Clock, Monotonic};
use smithay::wayland::presentation::Refresh;

/// The refresh rate assumed of an output that names none, in millihertz.
const DEFAULT_REFRESH_MILLIHERTZ: u64 = 60_000;

/// Returns the time between `output`'s refreshes, at its mode's rate.
pub(crate) fn frame_interval(output: &Output) -> Duration {
    let millihertz = output
        .current_mode()
        .and_then(|mode| u64::try_from(mode.refresh).ok())
        .filter(|millihertz| *millihertz > 0)
        .unwrap_or(DEFAULT_REFRESH_MILLIHERTZ);
    interval_at(millihertz)
}

/// Returns the time between refreshes at `millihertz`.
fn interval_at(millihertz: u64) -> Duration {
    Duration::from_nanos(1_000_000_000_000 / millihertz.max(1))
}

/// Returns when a monitor showed a frame, by the monotonic clock: as its
/// page flip reports, or `now` if it did not (or reported the wall-clock
/// time, which clients asked for monotonic time cannot use).
pub(crate) fn shown_at(metadata: Option<&DrmEventMetadata>, now: Duration) -> Duration {
    match metadata.map(|metadata| metadata.time) {
        Some(DrmEventTime::Monotonic(time)) => time,
        Some(DrmEventTime::Realtime(_)) | None => now,
    }
}

/// Returns the monotonic clock's time now.
pub(crate) fn now() -> Duration {
    Clock::<Monotonic>::new().now().into()
}

/// Tells the clients in `feedback` their frame showed on a monitor at
/// `time` (monotonic), its `sequence`th refresh, refreshing every
/// `interval` (at most that often, when its rate varies).
pub(crate) fn report_shown(
    mut feedback: OutputPresentationFeedback,
    time: Duration,
    sequence: u64,
    (interval, is_variable): (Duration, bool),
) {
    let refresh = if is_variable {
        Refresh::variable(interval)
    } else {
        Refresh::fixed(interval)
    };
    feedback.presented::<_, Monotonic>(
        time,
        refresh,
        sequence,
        Kind::Vsync | Kind::HwClock | Kind::HwCompletion,
    );
}

/// The frames an output with no page flip has shown, which number its
/// presentations.
#[derive(Debug, Default)]
pub(crate) struct FrameCounter {
    /// How many frames it has shown.
    shown: u64,
}

impl FrameCounter {
    /// Tells the clients in `feedback` their frame showed now on `output`.
    pub(crate) fn report_shown_now(
        &mut self,
        output: &Output,
        mut feedback: OutputPresentationFeedback,
    ) {
        self.shown = self.shown.wrapping_add(1);
        feedback.presented::<_, Monotonic>(
            now(),
            Refresh::fixed(frame_interval(output)),
            self.shown,
            Kind::empty(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interval_follows_the_refresh_rate() {
        assert_eq!(interval_at(60_000), Duration::from_nanos(16_666_666));
        assert_eq!(interval_at(144_000), Duration::from_nanos(6_944_444));
        assert_eq!(
            interval_at(0),
            Duration::from_secs(1000),
            "never divides by zero"
        );
    }

    #[test]
    fn an_output_without_a_mode_refreshes_at_60_hz() {
        let output = Output::new(
            "NONE".to_owned(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: String::new(),
                model: String::new(),
                serial_number: String::new(),
            },
        );
        assert_eq!(frame_interval(&output), interval_at(60_000));
    }

    #[test]
    fn a_frame_shows_when_the_monitor_says() {
        let now = Duration::from_secs(9);
        let flip = |time| DrmEventMetadata { time, sequence: 1 };
        let at = Duration::from_secs(5);
        assert_eq!(shown_at(Some(&flip(DrmEventTime::Monotonic(at))), now), at);
        assert_eq!(
            shown_at(
                Some(&flip(DrmEventTime::Realtime(std::time::SystemTime::now()))),
                now
            ),
            now,
            "wall-clock time is not the clients' clock"
        );
        assert_eq!(shown_at(None, now), now);
    }
}

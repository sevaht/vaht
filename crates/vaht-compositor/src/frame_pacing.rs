//! Frame pacing: `wp_fifo_v1` and `wp_commit_timing_v1`, which Vulkan's
//! FIFO present mode (Mesa), games and video players use.
//!
//! A client using fifo commits a frame that waits until its last frame has
//! been shown (a barrier); vaht lifts a surface's barrier when the output
//! showing it refreshes. A surface no output shows (a minimized game) is
//! released by any output's refresh: it stays paced, never stalled. A
//! client using commit timing asks that a frame show no sooner than a
//! moment; vaht lets it through when the next refresh would show it then,
//! and wakes for the earliest such moment it knows of.
//!
//! Which surfaces to release is decided here from plain values
//! ([`is_released_by`], [`wake_in`]); [`FramePacing`] applies it to
//! surfaces' state, and keeps the timers that wake vaht for timed frames.

use std::time::Duration;

use smithay::desktop::utils::{surface_primary_scanout_output, with_surfaces_surface_tree};
use smithay::output::Output;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource as _};
use smithay::utils::{Monotonic, Time};
use smithay::wayland::commit_timing::CommitTimerBarrierStateUserData;
use smithay::wayland::fifo::FifoBarrierCachedState;

use crate::Compositor;
use crate::event_source::Timer;
use crate::resources::ResourceLedger;

/// Returns whether a refresh of `refreshed` releases a surface `shown_on`
/// shows best: the same output, or none (the surface is hidden).
pub(crate) fn is_released_by<T: PartialEq>(shown_on: Option<&T>, refreshed: &T) -> bool {
    shown_on.is_none_or(|shown_on| shown_on == refreshed)
}

/// Returns how long from `now` to wake for a frame due to show at
/// `deadline`, on an output refreshing every `interval`: a refresh before
/// it, so the frame is drawn in time.
pub(crate) fn wake_in(deadline: Duration, now: Duration, interval: Duration) -> Duration {
    deadline.saturating_sub(interval).saturating_sub(now)
}

/// Releases surfaces' frame barriers (which are in the surfaces' own
/// state), and wakes vaht for frames due at a moment.
#[derive(Debug, Default)]
pub(crate) struct FramePacing {
    /// Wakes vaht to draw the earliest timed frame, if one waits.
    wake: Option<Timer<Compositor>>,
    /// Looks again for timed frames after commits that may have added one.
    check: Option<Timer<Compositor>>,
}

impl FramePacing {
    /// Arranges for [`Compositor::follow_timed_frames`] to look again for
    /// timed frames, once what is being dispatched is done: a commit of a
    /// surface with a commit timer may have added one.
    pub(crate) fn check_soon(
        &mut self,
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
    ) {
        if self.check.as_ref().is_some_and(|check| !check.has_fired()) {
            return;
        }
        match Timer::once(
            handle,
            ledger,
            Duration::ZERO,
            Compositor::follow_timed_frames,
        ) {
            Ok(check) => self.check = Some(check),
            Err(error) => tracing::warn!(%error, "could not look for timed frames"),
        }
    }

    /// Wakes vaht to draw a frame due at `deadline` (monotonic time) on an
    /// output refreshing every `interval`, if one is; forgets any earlier
    /// wake. Never called from the wake's own callback.
    pub(crate) fn wake_for(
        &mut self,
        deadline: Option<Duration>,
        interval: Duration,
        (handle, ledger): (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        self.wake = deadline.and_then(|deadline| {
            let delay = wake_in(deadline, crate::presentation::now(), interval);
            Timer::once(handle, ledger, delay, Compositor::schedule_render)
                .inspect_err(|error| tracing::warn!(%error, "could not wait for a timed frame"))
                .ok()
        });
    }

    /// Lifts the fifo barrier of each surface in the trees of `roots` that
    /// a refresh of `output` releases; returns the clients whose frames
    /// may now go on, once each.
    pub(crate) fn output_refreshed(roots: &[WlSurface], output: &Output) -> Vec<Client> {
        let mut released = Vec::new();
        for root in roots {
            with_surfaces_surface_tree(root, |surface, states| {
                let shown_on = surface_primary_scanout_output(surface, states);
                if !is_released_by(shown_on.as_ref(), output) {
                    return;
                }
                let barrier = states
                    .cached_state
                    .get::<FifoBarrierCachedState>()
                    .current()
                    .barrier
                    .take();
                if let Some(barrier) = barrier {
                    barrier.signal();
                    note_client(&mut released, surface);
                }
            });
        }
        released
    }

    /// Lets through each frame in the trees of `roots` due to show by
    /// `target` (monotonic time); returns the clients whose frames may
    /// now go on, once each.
    pub(crate) fn release_due(roots: &[WlSurface], target: Duration) -> Vec<Client> {
        let mut released = Vec::new();
        for root in roots {
            with_surfaces_surface_tree(root, |surface, states| {
                let Some(timers) = states.data_map.get::<CommitTimerBarrierStateUserData>() else {
                    return;
                };
                let Ok(mut timers) = timers.lock() else {
                    return;
                };
                if timers.signal_until(Time::<Monotonic>::from(target)) {
                    note_client(&mut released, surface);
                }
            });
        }
        released
    }

    /// Returns the earliest moment (monotonic time) a frame in the trees
    /// of `roots` waits for.
    pub(crate) fn next_deadline(roots: &[WlSurface]) -> Option<Duration> {
        let mut earliest: Option<Duration> = None;
        for root in roots {
            with_surfaces_surface_tree(root, |_, states| {
                let deadline = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .and_then(|timers| timers.lock().ok()?.next_deadline());
                if let Some(deadline) = deadline {
                    let deadline: Duration = Time::<Monotonic>::from(deadline).into();
                    earliest = Some(earliest.map_or(deadline, |known| known.min(deadline)));
                }
            });
        }
        earliest
    }
}

/// Adds `surface`'s client to `clients`, unless it is there.
fn note_client(clients: &mut Vec<Client>, surface: &WlSurface) {
    if let Some(client) = surface.client()
        && !clients.contains(&client)
    {
        clients.push(client);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refresh_releases_what_its_output_shows_and_what_none_shows() {
        assert!(is_released_by(Some(&"DP-1"), &"DP-1"));
        assert!(!is_released_by(Some(&"DP-2"), &"DP-1"), "another output's");
        assert!(is_released_by(None, &"DP-1"), "a hidden surface's");
    }

    #[test]
    fn a_timed_frame_is_drawn_a_refresh_early() {
        let interval = Duration::from_millis(16);
        let now = Duration::from_secs(10);
        assert_eq!(
            wake_in(now + Duration::from_millis(100), now, interval),
            Duration::from_millis(84)
        );
        assert_eq!(
            wake_in(now + Duration::from_millis(5), now, interval),
            Duration::ZERO,
            "due within a refresh: now"
        );
        assert_eq!(wake_in(Duration::ZERO, now, interval), Duration::ZERO);
    }
}

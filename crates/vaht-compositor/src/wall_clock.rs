//! Waking at a moment of the time of day, whatever happens to the clock.
//!
//! A [`WallClock`] calls its owner back at a moment it is told, and also
//! whenever the time of day moves on its own: when the clock is set (by
//! hand, or by NTP stepping it), after a suspend, or when the time zone
//! changes. What shows the time (the taskbar's clock) then works out its
//! text and its next moment from the time as it is, and is never stale.
//!
//! A monotonic timer, which calloop's timers are, cannot do this: it counts
//! elapsed time, so a clock set forward 20 seconds into a minute would show
//! the old minute until the rest of that minute had elapsed, and a suspend
//! stops it outright. Instead, a timerfd on `CLOCK_REALTIME` is armed at
//! the moment itself, so the kernel fires it at that time of day however
//! the clock got there; `TFD_TIMER_CANCEL_ON_SET` makes it fire too when
//! the clock is set, which a moment that moved further away (the clock
//! set back) needs. Time zone changes replace `/etc/localtime`, which an
//! inotify watch on its directory sees; jiff, which otherwise checks for a
//! new zone only every few minutes, is then told to read it again.

use std::path::Path;
use std::rc::Rc;

use jiff::Timestamp;
use nix::errno::Errno;
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use nix::sys::time::TimeSpec;
use nix::sys::timerfd::{ClockId, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};

use crate::event_source::EventSource;
use crate::resources::ResourceLedger;

/// The file whose replacement changes the system's time zone.
pub(crate) const SYSTEM_ZONE_FILE: &str = "/etc/localtime";

/// Wakes its owner at a moment, and whenever the time of day jumps.
#[derive(Debug)]
pub(crate) struct WallClock<State: 'static> {
    /// The alarm's registration; dropped before the alarm it reads.
    _alarm_source: EventSource<State>,
    /// The time zone file's watch, when its directory can be watched.
    _zone_watch: Option<EventSource<State>>,
    /// The alarm, armed at the next moment.
    alarm: Rc<TimerFd>,
}

impl<State: 'static> WallClock<State> {
    /// What replacing or rewriting a file in a directory looks like.
    const ZONE_EVENTS: AddWatchFlags = AddWatchFlags::IN_CLOSE_WRITE
        .union(AddWatchFlags::IN_MOVED_TO)
        .union(AddWatchFlags::IN_CREATE)
        .union(AddWatchFlags::IN_DELETE);

    /// Starts a clock that calls `on_change` at each moment it is armed
    /// for, and whenever the clock is set or `zone_file` (normally
    /// [`SYSTEM_ZONE_FILE`]) is replaced. A zone file that cannot be watched
    /// is logged, and only zone changes then go unseen.
    ///
    /// # Errors
    ///
    /// Returns an error if the alarm cannot be created or registered.
    pub(crate) fn start(
        handle: &LoopHandle<'static, State>,
        ledger: &ResourceLedger,
        zone_file: &Path,
        on_change: fn(&mut State),
    ) -> Result<Self, WallClockError> {
        let alarm = Rc::new(
            TimerFd::new(
                ClockId::CLOCK_REALTIME,
                TimerFlags::TFD_NONBLOCK | TimerFlags::TFD_CLOEXEC,
            )
            .map_err(WallClockError::Create)?,
        );
        let alarm_source = EventSource::insert(
            handle,
            ledger,
            Generic::new(Rc::clone(&alarm), Interest::READ, Mode::Level),
            move |_, alarm, state| {
                // Reading reports the alarm ringing, and also the clock
                // being set (as ECANCELED, which `wait` takes as a ring);
                // EAGAIN is a wake-up with neither.
                match alarm.wait() {
                    Ok(()) => on_change(state),
                    Err(Errno::EAGAIN) => {}
                    Err(error) => tracing::warn!(%error, "could not read the clock's alarm"),
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| WallClockError::Register(error.error))?;
        let zone_watch = Self::watch_zone(handle, ledger, zone_file, on_change)
            .inspect_err(|error| {
                tracing::warn!(
                    error = error as &dyn std::error::Error,
                    file = %zone_file.display(),
                    "time zone changes will show only within minutes"
                );
            })
            .ok();
        Ok(Self {
            _alarm_source: alarm_source,
            _zone_watch: zone_watch,
            alarm,
        })
    }

    /// Arms the clock to call back at `moment`, in place of any moment
    /// before; a moment already past calls back at once.
    ///
    /// # Errors
    ///
    /// Returns an error if the kernel refuses the alarm.
    pub(crate) fn ring_at(&self, moment: Timestamp) -> Result<(), WallClockError> {
        let at = TimeSpec::new(moment.as_second(), i64::from(moment.subsec_nanosecond()));
        self.alarm
            .set(
                Expiration::OneShot(at),
                TimerSetTimeFlags::TFD_TIMER_ABSTIME | TimerSetTimeFlags::TFD_TIMER_CANCEL_ON_SET,
            )
            .map_err(WallClockError::Arm)
    }

    /// Watches the directory of `zone_file` for the file being replaced.
    fn watch_zone(
        handle: &LoopHandle<'static, State>,
        ledger: &ResourceLedger,
        zone_file: &Path,
        on_change: fn(&mut State),
    ) -> Result<EventSource<State>, WallClockError> {
        let (Some(directory), Some(name)) = (zone_file.parent(), zone_file.file_name()) else {
            return Err(WallClockError::Watch(Errno::EINVAL));
        };
        let name = name.to_owned();
        let inotify = Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC)
            .map_err(WallClockError::Watch)?;
        inotify
            .add_watch(directory, Self::ZONE_EVENTS)
            .map_err(WallClockError::Watch)?;
        EventSource::insert(
            handle,
            ledger,
            Generic::new(inotify, Interest::READ, Mode::Level),
            move |_, inotify, state| {
                let events = match inotify.read_events() {
                    Ok(events) => events,
                    Err(Errno::EAGAIN) => return Ok(PostAction::Continue),
                    Err(error) => {
                        tracing::warn!(%error, "could not read time zone changes");
                        return Ok(PostAction::Continue);
                    }
                };
                if events
                    .iter()
                    .any(|event| event.name.as_deref() == Some(name.as_os_str()))
                {
                    jiff::tz::reset_system_time_zone_cache();
                    on_change(state);
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| WallClockError::Register(error.error))
    }
}

/// Why a [`WallClock`] could not start or be armed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WallClockError {
    /// The alarm could not be created.
    #[error("could not create a timerfd")]
    Create(#[source] Errno),
    /// The alarm could not be armed.
    #[error("could not arm the timerfd")]
    Arm(#[source] Errno),
    /// The time zone file's directory could not be watched.
    #[error("could not watch the time zone file")]
    Watch(#[source] Errno),
    /// A source could not be registered with the event loop.
    #[error("could not register with the event loop")]
    Register(#[source] calloop::Error),
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use calloop::EventLoop;

    use super::*;

    /// How many times the clock called back.
    type Calls = u32;

    /// How long a test waits for a call it expects.
    const WAIT: Duration = Duration::from_millis(200);

    /// Counts a call.
    fn count(calls: &mut Calls) {
        *calls += 1;
    }

    /// Dispatches `event_loop` until `calls` reaches `expected`, or a while
    /// has passed.
    fn dispatch_until(
        event_loop: &mut EventLoop<'static, Calls>,
        calls: &mut Calls,
        expected: u32,
    ) {
        for _ in 0..10 {
            if *calls >= expected {
                break;
            }
            event_loop.dispatch(WAIT, calls).expect("dispatch");
        }
    }

    #[test]
    fn the_clock_calls_back_at_its_moment_and_not_before() {
        let mut event_loop: EventLoop<'static, Calls> = EventLoop::try_new().expect("a loop");
        let ledger = ResourceLedger::new();
        let zone = tempfile::tempdir().expect("a temporary directory");
        let clock = WallClock::start(
            &event_loop.handle(),
            &ledger,
            &zone.path().join("localtime"),
            count,
        )
        .expect("started");
        let mut calls = 0;
        event_loop
            .dispatch(Duration::from_millis(20), &mut calls)
            .expect("dispatch");
        assert_eq!(calls, 0, "unarmed, it stays quiet");

        let moment = Timestamp::now() + Duration::from_millis(50);
        clock.ring_at(moment).expect("armed");
        dispatch_until(&mut event_loop, &mut calls, 1);
        assert_eq!(calls, 1);
        assert!(Timestamp::now() >= moment, "never early");

        clock
            .ring_at(Timestamp::now() + Duration::from_millis(5))
            .expect("armed again");
        dispatch_until(&mut event_loop, &mut calls, 2);
        assert_eq!(calls, 2, "it can be armed again");
    }

    #[test]
    fn replacing_the_zone_file_calls_back() {
        let mut event_loop: EventLoop<'static, Calls> = EventLoop::try_new().expect("a loop");
        let ledger = ResourceLedger::new();
        let zone = tempfile::tempdir().expect("a temporary directory");
        let zone_file = zone.path().join("localtime");
        let _clock =
            WallClock::start(&event_loop.handle(), &ledger, &zone_file, count).expect("started");
        let mut calls = 0;
        std::fs::write(zone.path().join("unrelated"), "x").expect("written");
        event_loop
            .dispatch(Duration::from_millis(20), &mut calls)
            .expect("dispatch");
        assert_eq!(calls, 0, "other files are not the zone");

        std::fs::write(zone.path().join("new-zone"), "x").expect("written");
        std::fs::rename(zone.path().join("new-zone"), &zone_file).expect("renamed");
        dispatch_until(&mut event_loop, &mut calls, 1);
        assert!(calls >= 1, "replacing the zone file is a change");
    }

    #[test]
    fn dropping_the_clock_unregisters_it() {
        let event_loop: EventLoop<'static, Calls> = EventLoop::try_new().expect("a loop");
        let ledger = ResourceLedger::new();
        let zone = tempfile::tempdir().expect("a temporary directory");
        let clock = WallClock::start(
            &event_loop.handle(),
            &ledger,
            &zone.path().join("localtime"),
            count,
        )
        .expect("started");
        assert_eq!(
            ledger
                .count(crate::resources::ResourceKind::EventSource)
                .live,
            2
        );
        drop(clock);
        assert_eq!(
            ledger
                .count(crate::resources::ResourceKind::EventSource)
                .live,
            0
        );
    }
}

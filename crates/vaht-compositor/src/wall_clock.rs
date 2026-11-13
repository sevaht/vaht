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
//! set back) needs.
//!
//! A time zone change is seen two ways ([`ZoneSources`]), so that either
//! alone suffices: timedated, which `timedatectl` and desktop settings ask
//! to change it, announces it on the system bus; and every change, whoever
//! makes it, replaces `/etc/localtime`, which an inotify watch on its
//! directory sees. jiff, which otherwise checks for a new zone only every
//! few minutes, is then told to read it again.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures_util::{Stream, StreamExt};
use jiff::Timestamp;
use nix::errno::Errno;
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use nix::sys::time::TimeSpec;
use nix::sys::timerfd::{ClockId, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};

use crate::bus::Bus;
use crate::event_source::{EventSource, StreamSource};
use crate::resources::ResourceLedger;

/// Where a change of the system's time zone shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ZoneSources {
    /// The file a new zone replaces.
    pub(crate) file: PathBuf,
    /// The bus timedated announces a new zone on.
    pub(crate) bus: Bus,
}

impl ZoneSources {
    /// timedated's bus name, which is also the interface of its properties.
    const TIMEDATED: &str = "org.freedesktop.timedate1";
    /// timedated's object.
    const TIMEDATED_PATH: &str = "/org/freedesktop/timedate1";
    /// timedated's property holding the zone.
    const ZONE_PROPERTY: &str = "Timezone";

    /// The system's: `/etc/localtime`, and timedated on the system bus.
    pub(crate) fn system() -> Self {
        Self {
            file: PathBuf::from("/etc/localtime"),
            bus: Bus::System,
        }
    }

    /// Returns whether a `PropertiesChanged` signal of `interface`, with
    /// `changed` and `invalidated` properties, announces a new zone.
    fn is_zone_change<'name>(
        interface: &str,
        mut changed: impl Iterator<Item = &'name str>,
        mut invalidated: impl Iterator<Item = &'name str>,
    ) -> bool {
        interface == Self::TIMEDATED
            && (changed.any(|name| name == Self::ZONE_PROPERTY)
                || invalidated.any(|name| name == Self::ZONE_PROPERTY))
    }

    /// Returns timedated's announcements of a new zone, once subscribed;
    /// none if the bus cannot be reached, which is logged.
    fn announcements(&self) -> impl Stream<Item = ()> + 'static {
        let bus = self.bus.clone();
        futures_util::stream::once(async move { Self::subscribe(&bus).await })
            .filter_map(|subscribed| async move {
                subscribed
                    .inspect_err(|error| {
                        tracing::info!(
                            error = error as &dyn std::error::Error,
                            "timedated cannot be heard; time zone changes show by /etc/localtime"
                        );
                    })
                    .ok()
            })
            .flatten()
    }

    /// Subscribes to timedated's property changes on `bus`, keeping those
    /// that change the zone.
    async fn subscribe(bus: &Bus) -> zbus::Result<impl Stream<Item = ()> + 'static + use<>> {
        let connection = bus.connect().await?;
        let properties = zbus::fdo::PropertiesProxy::builder(&connection)
            .destination(Self::TIMEDATED)
            .path(Self::TIMEDATED_PATH)
            .build()
            .await?;
        let changes = properties.receive_properties_changed().await?;
        Ok(changes.filter_map(|signal| async move {
            let arguments = signal.args().ok()?;
            Self::is_zone_change(
                arguments.interface_name().as_str(),
                arguments.changed_properties().keys().copied(),
                arguments.invalidated_properties().iter().copied(),
            )
            .then_some(())
        }))
    }
}

/// Wakes its owner at a moment, and whenever the time of day jumps.
#[derive(Debug)]
pub(crate) struct WallClock<State: 'static> {
    /// The alarm's registration; dropped before the alarm it reads.
    _alarm_source: EventSource<State>,
    /// The time zone file's watch, when its directory can be watched.
    _zone_watch: Option<EventSource<State>>,
    /// timedated's announcements of a new zone.
    _zone_announcements: StreamSource<State>,
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
    /// for, and whenever the clock is set or `zone` (normally
    /// [`ZoneSources::system`]) shows a new time zone. A zone source that
    /// cannot be watched is logged, and the other still serves.
    ///
    /// # Errors
    ///
    /// Returns an error if the alarm cannot be created or registered.
    pub(crate) fn start(
        handle: &LoopHandle<'static, State>,
        ledger: &ResourceLedger,
        zone: &ZoneSources,
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
        let zone_watch = Self::watch_zone(handle, ledger, &zone.file, on_change)
            .inspect_err(|error| {
                tracing::warn!(
                    error = error as &dyn std::error::Error,
                    file = %zone.file.display(),
                    "time zone changes show only by timedated"
                );
            })
            .ok();
        let zone_announcements =
            StreamSource::start(handle, ledger, zone.announcements(), move |(), state| {
                jiff::tz::reset_system_time_zone_cache();
                on_change(state);
            })
            .map_err(WallClockError::Register)?;
        Ok(Self {
            _alarm_source: alarm_source,
            _zone_watch: zone_watch,
            _zone_announcements: zone_announcements,
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
    use std::collections::HashMap;
    use std::time::Duration;

    use calloop::EventLoop;
    use rstest::rstest;
    use zbus::Variant;

    use super::*;
    use crate::bus::PrivateBus;
    use crate::resources::ResourceKind;

    /// What a test's event loop holds.
    #[derive(Debug, Default)]
    struct Probe {
        /// How many times the clock called back.
        calls: u32,
        /// The connection playing timedated, once connected.
        timedated: Option<zbus::Connection>,
    }

    /// How long one dispatch waits for events.
    const WAIT: Duration = Duration::from_millis(50);

    /// How long a test waits for a call it expects.
    const PATIENCE: Duration = Duration::from_secs(5);

    /// Counts a call.
    fn count(probe: &mut Probe) {
        probe.calls += 1;
    }

    /// Zone sources in `directory`, on a bus of the test's own.
    fn sources(directory: &Path, bus: &PrivateBus) -> ZoneSources {
        ZoneSources {
            file: directory.join("localtime"),
            bus: bus.bus(),
        }
    }

    /// Dispatches `event_loop` until the clock has called back `expected`
    /// times, or a while has passed.
    fn dispatch_until(
        event_loop: &mut EventLoop<'static, Probe>,
        probe: &mut Probe,
        expected: u32,
    ) {
        let deadline = std::time::Instant::now() + PATIENCE;
        while probe.calls < expected && std::time::Instant::now() < deadline {
            event_loop.dispatch(WAIT, probe).expect("dispatch");
        }
    }

    #[test]
    fn the_clock_calls_back_at_its_moment_and_not_before() {
        let mut event_loop: EventLoop<'static, Probe> = EventLoop::try_new().expect("a loop");
        let ledger = ResourceLedger::new();
        let zone = tempfile::tempdir().expect("a temporary directory");
        let bus = PrivateBus::start();
        let clock = WallClock::start(
            &event_loop.handle(),
            &ledger,
            &sources(zone.path(), &bus),
            count,
        )
        .expect("started");
        let mut probe = Probe::default();
        event_loop.dispatch(WAIT, &mut probe).expect("dispatch");
        assert_eq!(probe.calls, 0, "unarmed, it stays quiet");

        let moment = Timestamp::now() + Duration::from_millis(50);
        clock.ring_at(moment).expect("armed");
        dispatch_until(&mut event_loop, &mut probe, 1);
        assert_eq!(probe.calls, 1);
        assert!(Timestamp::now() >= moment, "never early");

        clock
            .ring_at(Timestamp::now() + Duration::from_millis(5))
            .expect("armed again");
        dispatch_until(&mut event_loop, &mut probe, 2);
        assert_eq!(probe.calls, 2, "it can be armed again");
    }

    #[test]
    fn replacing_the_zone_file_calls_back() {
        let mut event_loop: EventLoop<'static, Probe> = EventLoop::try_new().expect("a loop");
        let ledger = ResourceLedger::new();
        let zone = tempfile::tempdir().expect("a temporary directory");
        let bus = PrivateBus::start();
        let sources = sources(zone.path(), &bus);
        let _clock =
            WallClock::start(&event_loop.handle(), &ledger, &sources, count).expect("started");
        let mut probe = Probe::default();
        std::fs::write(zone.path().join("unrelated"), "x").expect("written");
        event_loop.dispatch(WAIT, &mut probe).expect("dispatch");
        assert_eq!(probe.calls, 0, "other files are not the zone");

        std::fs::write(zone.path().join("new-zone"), "x").expect("written");
        std::fs::rename(zone.path().join("new-zone"), &sources.file).expect("renamed");
        dispatch_until(&mut event_loop, &mut probe, 1);
        assert!(probe.calls >= 1, "replacing the zone file is a change");
    }

    #[test]
    fn timedated_announcing_a_new_zone_calls_back() {
        let mut event_loop: EventLoop<'static, Probe> = EventLoop::try_new().expect("a loop");
        let ledger = ResourceLedger::new();
        let zone = tempfile::tempdir().expect("a temporary directory");
        let bus = PrivateBus::start();
        let _clock = WallClock::start(
            &event_loop.handle(),
            &ledger,
            &sources(zone.path(), &bus),
            count,
        )
        .expect("started");
        // timedated starts after vaht subscribed, as it does when the bus
        // activates it to change the zone. It connects, then announces
        // until heard, as vaht's subscription may not be in place yet.
        let (executor, scheduler) = calloop::futures::executor().expect("an executor");
        let _timedated = EventSource::insert(
            &event_loop.handle(),
            &ledger,
            executor,
            |connected: Option<zbus::Connection>, (), probe: &mut Probe| {
                if connected.is_some() {
                    probe.timedated = connected;
                }
            },
        )
        .expect("registered");
        let timedated_bus = bus.bus();
        scheduler
            .schedule(async move {
                let connection = timedated_bus.connect().await.expect("connected");
                connection
                    .request_name(ZoneSources::TIMEDATED)
                    .await
                    .expect("named");
                Some(connection)
            })
            .expect("scheduled");
        let mut probe = Probe::default();
        let deadline = std::time::Instant::now() + PATIENCE;
        while probe.calls == 0 && std::time::Instant::now() < deadline {
            if let Some(connection) = probe.timedated.clone() {
                scheduler
                    .schedule(async move {
                        let changed = HashMap::from([(
                            ZoneSources::ZONE_PROPERTY,
                            Variant::from("Europe/Oslo"),
                        )]);
                        connection
                            .emit_signal(
                                None::<&str>,
                                ZoneSources::TIMEDATED_PATH,
                                "org.freedesktop.DBus.Properties",
                                "PropertiesChanged",
                                &(ZoneSources::TIMEDATED, changed, Vec::<&str>::new()),
                            )
                            .await
                            .expect("announced");
                        None
                    })
                    .expect("scheduled");
            }
            event_loop.dispatch(WAIT, &mut probe).expect("dispatch");
        }
        assert!(probe.calls >= 1, "the announcement was heard");
    }

    #[rstest]
    #[case::the_zone_changed(ZoneSources::TIMEDATED, &["Timezone"], &[], true)]
    #[case::the_zone_was_invalidated(ZoneSources::TIMEDATED, &[], &["Timezone"], true)]
    #[case::another_property(ZoneSources::TIMEDATED, &["NTP"], &[], false)]
    #[case::another_interface("org.example.other", &["Timezone"], &[], false)]
    fn only_timedated_s_zone_property_is_a_zone_change(
        #[case] interface: &str,
        #[case] changed: &[&str],
        #[case] invalidated: &[&str],
        #[case] expected: bool,
    ) {
        assert_eq!(
            ZoneSources::is_zone_change(
                interface,
                changed.iter().copied(),
                invalidated.iter().copied()
            ),
            expected
        );
    }

    #[test]
    fn dropping_the_clock_unregisters_it() {
        let event_loop: EventLoop<'static, Probe> = EventLoop::try_new().expect("a loop");
        let ledger = ResourceLedger::new();
        let zone = tempfile::tempdir().expect("a temporary directory");
        let bus = PrivateBus::start();
        let clock = WallClock::start(
            &event_loop.handle(),
            &ledger,
            &sources(zone.path(), &bus),
            count,
        )
        .expect("started");
        assert_eq!(ledger.count(ResourceKind::EventSource).live, 3);
        drop(clock);
        assert_eq!(ledger.count(ResourceKind::EventSource).live, 0);
    }
}

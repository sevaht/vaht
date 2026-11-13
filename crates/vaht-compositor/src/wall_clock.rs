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
//! A time zone change is heard from timedated, which `timedatectl` and
//! desktop settings ask to change it, and which announces it on the system
//! bus, while timedated runs. While it does not (it exits when idle, and
//! the bus starts it again on demand), or once the bus is lost, the clock
//! watches the zone file (`/etc/localtime`, which every change replaces)
//! instead, and from the start, until timedated is known to run: one
//! source at a time, so vaht is not woken by writes to `/etc` while
//! timedated speaks for the zone. Nothing is polled: the bus
//! announces timedated starting and stopping (`NameOwnerChanged`), and the
//! subscription to its news stays in place across both. jiff, which
//! otherwise checks for a new zone only every few minutes, is then told to
//! read it again.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures_util::future::{Either, ready};
use futures_util::{Stream, StreamExt, stream};
use jiff::Timestamp;
use nix::errno::Errno;
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use nix::sys::time::TimeSpec;
use nix::sys::timerfd::{ClockId, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
use zbus::fdo::{DBusProxy, PropertiesProxy};

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

    /// Returns what is heard of timedated: whether it runs, then each new
    /// zone and each start and stop, then that it can no longer be heard,
    /// if the bus goes; or only that, if the bus cannot be reached, which
    /// is logged.
    fn news(&self) -> impl Stream<Item = ZoneNews> + 'static {
        let bus = self.bus.clone();
        stream::once(async move { Self::subscribe(&bus).await }).flat_map(|subscribed| {
            match subscribed {
                Ok(news) => Either::Left(news.chain(stream::once(ready(ZoneNews::Unheard)))),
                Err(error) => {
                    tracing::info!(
                        error = &error as &dyn std::error::Error,
                        "timedated cannot be heard; watching the zone file instead"
                    );
                    Either::Right(stream::once(ready(ZoneNews::Unheard)))
                }
            }
        })
    }

    /// Subscribes on `bus` to timedated starting and stopping, and to its
    /// property changes that change the zone; the news starts with whether
    /// it runs now.
    async fn subscribe(bus: &Bus) -> zbus::Result<impl Stream<Item = ZoneNews> + 'static + use<>> {
        let connection = bus.connect().await?;
        let daemon = DBusProxy::new(&connection).await?;
        // Subscribed before asking, so a start or stop in between is heard.
        let owners = daemon
            .receive_name_owner_changed_with_args(&[(0, Self::TIMEDATED)])
            .await?
            .filter_map(|signal| async move {
                let arguments = signal.args().ok()?;
                Some(if arguments.new_owner().is_some() {
                    ZoneNews::Started
                } else {
                    ZoneNews::Stopped
                })
            });
        let properties = PropertiesProxy::builder(&connection)
            .destination(Self::TIMEDATED)
            .path(Self::TIMEDATED_PATH)
            // The proxy would otherwise follow the name's owner itself.
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await?;
        let changes =
            properties
                .receive_properties_changed()
                .await?
                .filter_map(|signal| async move {
                    let arguments = signal.args().ok()?;
                    Self::is_zone_change(
                        arguments.interface_name().as_str(),
                        arguments.changed_properties().keys().copied(),
                        arguments.invalidated_properties().iter().copied(),
                    )
                    .then_some(ZoneNews::Changed)
                });
        let is_running = daemon.name_has_owner(Self::TIMEDATED.try_into()?).await?;
        let now = if is_running {
            ZoneNews::Started
        } else {
            ZoneNews::Stopped
        };
        Ok(stream::once(ready(now)).chain(stream::select(owners, changes)))
    }
}

/// What is heard of timedated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZoneNews {
    /// It runs: it speaks for the zone.
    Started,
    /// It stopped, or was not running.
    Stopped,
    /// It announced a new zone.
    Changed,
    /// The bus is gone, or could not be reached.
    Unheard,
}

/// How a [`WallClock`] reaches its owner: what to call when the time of
/// day changed, and how to find the clock again in the owner's state, as
/// the clock changes how it listens.
pub(crate) struct ClockHooks<State: 'static> {
    /// Called when the time shown may have changed.
    pub(crate) on_change: fn(&mut State),
    /// Finds the clock in the state, while it exists.
    pub(crate) find: fn(&mut State) -> Option<&mut WallClock<State>>,
}

impl<State> Clone for ClockHooks<State> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<State> Copy for ClockHooks<State> {}

impl<State> std::fmt::Debug for ClockHooks<State> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ClockHooks").finish_non_exhaustive()
    }
}

/// Wakes its owner at a moment, and whenever the time of day jumps.
#[derive(Debug)]
pub(crate) struct WallClock<State: 'static> {
    /// The alarm's registration; dropped before the alarm it reads.
    _alarm_source: EventSource<State>,
    /// What timedated says.
    _zone_news: StreamSource<State>,
    /// The zone file's watch, while timedated does not speak for the zone.
    zone_watch: Option<EventSource<State>>,
    /// The alarm, armed at the next moment.
    alarm: Rc<TimerFd>,
    /// The file a new zone replaces.
    zone_file: PathBuf,
    /// The loop the clock's sources are registered with.
    handle: LoopHandle<'static, State>,
    /// Counts the clock's sources.
    ledger: ResourceLedger,
    /// How the clock reaches its owner.
    hooks: ClockHooks<State>,
}

impl<State: 'static> WallClock<State> {
    /// What replacing or rewriting a file in a directory looks like.
    const ZONE_EVENTS: AddWatchFlags = AddWatchFlags::IN_CLOSE_WRITE
        .union(AddWatchFlags::IN_MOVED_TO)
        .union(AddWatchFlags::IN_CREATE)
        .union(AddWatchFlags::IN_DELETE);

    /// Starts a clock that calls `hooks.on_change` at each moment it is
    /// armed for, and whenever the clock is set or `zone` (normally
    /// [`ZoneSources::system`]) shows a new time zone.
    ///
    /// # Errors
    ///
    /// Returns an error if the alarm cannot be created or registered.
    pub(crate) fn start(
        handle: &LoopHandle<'static, State>,
        ledger: &ResourceLedger,
        zone: &ZoneSources,
        hooks: ClockHooks<State>,
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
                    Ok(()) => (hooks.on_change)(state),
                    Err(Errno::EAGAIN) => {}
                    Err(error) => tracing::warn!(%error, "could not read the clock's alarm"),
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| WallClockError::Register(error.error))?;
        let zone_news = StreamSource::start(handle, ledger, zone.news(), move |news, state| {
            if let Some(clock) = (hooks.find)(state) {
                clock.hear(news);
            }
            if matches!(news, ZoneNews::Changed | ZoneNews::Unheard) {
                // A new zone, or whatever was missed as the bus went.
                jiff::tz::reset_system_time_zone_cache();
                (hooks.on_change)(state);
            }
        })
        .map_err(WallClockError::Register)?;
        let mut clock = Self {
            _alarm_source: alarm_source,
            _zone_news: zone_news,
            zone_watch: None,
            alarm,
            zone_file: zone.file.clone(),
            handle: handle.clone(),
            ledger: ledger.clone(),
            hooks,
        };
        // Until timedated is known to run, the file speaks for the zone, so
        // no change goes unseen while the bus is asked.
        clock.watch_zone_file();
        Ok(clock)
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

    /// Stops ringing at the moment armed: what shows the time is hidden.
    /// The clock being set or the zone changing still calls back.
    ///
    /// # Errors
    ///
    /// Returns an error if the kernel refuses.
    pub(crate) fn disarm(&self) -> Result<(), WallClockError> {
        self.alarm.unset().map_err(WallClockError::Arm)
    }

    /// Watches the zone file while timedated does not speak for the zone.
    fn hear(&mut self, news: ZoneNews) {
        match news {
            ZoneNews::Started => {
                if self.zone_watch.take().is_some() {
                    tracing::debug!("timedated runs; the zone file is left alone");
                }
            }
            ZoneNews::Stopped | ZoneNews::Unheard => self.watch_zone_file(),
            ZoneNews::Changed => {}
        }
    }

    /// Starts watching the zone file, unless it is watched already; a file
    /// that cannot be watched is logged, and zone changes then show within
    /// minutes, when jiff looks again.
    fn watch_zone_file(&mut self) {
        if self.zone_watch.is_some() {
            return;
        }
        match Self::watch_zone(
            &self.handle,
            &self.ledger,
            &self.zone_file,
            self.hooks.on_change,
        ) {
            Ok(watch) => self.zone_watch = Some(watch),
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                file = %self.zone_file.display(),
                "time zone changes will show only within minutes"
            ),
        }
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
    use std::time::{Duration, Instant};

    use calloop::EventLoop;
    use rstest::rstest;
    use zbus::Variant;

    use super::*;
    use crate::bus::PrivateBus;

    /// What a test's event loop holds.
    #[derive(Debug, Default)]
    struct Probe {
        /// How many times the clock called back.
        calls: u32,
        /// The clock under test.
        clock: Option<WallClock<Probe>>,
        /// The connection playing timedated, once connected.
        timedated: Option<zbus::Connection>,
    }

    /// How long one dispatch waits for events.
    const WAIT: Duration = Duration::from_millis(50);

    /// How long a test waits for something it expects.
    const PATIENCE: Duration = Duration::from_secs(5);

    /// The hooks that count calls and find the probe's clock.
    const HOOKS: ClockHooks<Probe> = ClockHooks {
        on_change: |probe| probe.calls += 1,
        find: |probe| probe.clock.as_mut(),
    };

    /// A clock's world: a loop, a bus of its own, a zone file's directory.
    struct World {
        /// The loop the clock runs on.
        event_loop: EventLoop<'static, Probe>,
        /// Counts the clock's sources, and nothing else.
        ledger: ResourceLedger,
        /// The bus timedated may be on.
        bus: PrivateBus,
        /// Where the zone file is.
        zone: tempfile::TempDir,
        /// What the loop holds.
        probe: Probe,
        /// Runs the test's own futures, playing timedated.
        timedated: calloop::futures::Scheduler<Option<zbus::Connection>>,
        /// Keeps the test's executor registered.
        _executor: EventSource<Probe>,
    }

    impl World {
        /// A world with no clock yet, and no timedated.
        fn new() -> Self {
            let event_loop: EventLoop<'static, Probe> = EventLoop::try_new().expect("a loop");
            let (executor, timedated) = calloop::futures::executor().expect("an executor");
            let executor = EventSource::insert(
                &event_loop.handle(),
                &ResourceLedger::new(),
                executor,
                |connected: Option<zbus::Connection>, (), probe: &mut Probe| {
                    if connected.is_some() {
                        probe.timedated = connected;
                    }
                },
            )
            .expect("registered");
            Self {
                event_loop,
                ledger: ResourceLedger::new(),
                bus: PrivateBus::start(),
                zone: tempfile::tempdir().expect("a temporary directory"),
                probe: Probe::default(),
                timedated,
                _executor: executor,
            }
        }

        /// Returns the zone file.
        fn zone_file(&self) -> PathBuf {
            self.zone.path().join("localtime")
        }

        /// Starts the clock.
        fn start_clock(&mut self) {
            let sources = ZoneSources {
                file: self.zone_file(),
                bus: self.bus.bus(),
            };
            let clock = WallClock::start(&self.event_loop.handle(), &self.ledger, &sources, HOOKS)
                .expect("started");
            self.probe.clock = Some(clock);
        }

        /// Puts timedated on the bus, and waits until it is.
        fn start_timedated(&mut self) {
            let bus = self.bus.bus();
            self.timedated
                .schedule(async move {
                    let connection = bus.connect().await.expect("connected");
                    connection
                        .request_name(ZoneSources::TIMEDATED)
                        .await
                        .expect("named");
                    Some(connection)
                })
                .expect("scheduled");
            self.dispatch_until(|probe| probe.timedated.is_some());
        }

        /// Has timedated announce a new zone.
        fn announce_zone(&self) {
            let connection = self
                .probe
                .timedated
                .clone()
                .expect("timedated is on the bus");
            self.timedated
                .schedule(async move {
                    let changed =
                        HashMap::from([(ZoneSources::ZONE_PROPERTY, Variant::from("Europe/Oslo"))]);
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

        /// Replaces the zone file, as a new zone does.
        fn replace_zone_file(&self) {
            let staged = self.zone.path().join("new-zone");
            std::fs::write(&staged, "x").expect("written");
            std::fs::rename(&staged, self.zone_file()).expect("renamed");
        }

        /// Returns whether the clock watches the zone file.
        fn watches_zone_file(&self) -> bool {
            self.probe
                .clock
                .as_ref()
                .is_some_and(|clock| clock.zone_watch.is_some())
        }

        /// Dispatches until `done` holds of the probe, or a while has passed.
        fn dispatch_until(&mut self, done: impl Fn(&Probe) -> bool) {
            let deadline = Instant::now() + PATIENCE;
            while !done(&self.probe) && Instant::now() < deadline {
                self.event_loop
                    .dispatch(WAIT, &mut self.probe)
                    .expect("dispatch");
            }
        }

        /// Dispatches for a moment, to let anything pending happen.
        fn settle(&mut self) {
            for _ in 0..4 {
                self.event_loop
                    .dispatch(WAIT, &mut self.probe)
                    .expect("dispatch");
            }
        }
    }

    #[test]
    fn the_clock_calls_back_at_its_moment_and_not_before() {
        let mut world = World::new();
        world.start_clock();
        // Without timedated, the clock settles on the zone file first, and
        // calls back once for that.
        world.dispatch_until(|probe| {
            probe
                .clock
                .as_ref()
                .is_some_and(|clock| clock.zone_watch.is_some())
        });
        world.settle();
        let calls = world.probe.calls;
        let moment = Timestamp::now() + Duration::from_millis(50);
        world
            .probe
            .clock
            .as_ref()
            .expect("started")
            .ring_at(moment)
            .expect("armed");
        world.dispatch_until(|probe| probe.calls > calls);
        assert_eq!(world.probe.calls, calls + 1);
        assert!(Timestamp::now() >= moment, "never early");
        world
            .probe
            .clock
            .as_ref()
            .expect("started")
            .ring_at(Timestamp::now() + Duration::from_millis(5))
            .expect("armed again");
        world.dispatch_until(|probe| probe.calls > calls + 1);
        assert_eq!(world.probe.calls, calls + 2, "it can be armed again");
    }

    #[test]
    fn with_timedated_its_news_is_heard_and_the_zone_file_left_alone() {
        let mut world = World::new();
        world.start_timedated();
        world.start_clock();
        assert!(world.watches_zone_file(), "until timedated is known to run");
        world.dispatch_until(|probe| {
            probe
                .clock
                .as_ref()
                .is_some_and(|clock| clock.zone_watch.is_none())
        });
        assert!(!world.watches_zone_file(), "no file watch beside timedated");
        // Announce until heard: the subscription may not be in place yet.
        let deadline = Instant::now() + PATIENCE;
        while world.probe.calls == 0 && Instant::now() < deadline {
            world.announce_zone();
            world.settle();
        }
        assert!(world.probe.calls >= 1, "the announcement was heard");
        assert!(!world.watches_zone_file());
    }

    #[test]
    fn without_timedated_the_zone_file_is_watched() {
        let mut world = World::new();
        world.start_clock();
        world.dispatch_until(|probe| {
            probe
                .clock
                .as_ref()
                .is_some_and(|clock| clock.zone_watch.is_some())
        });
        assert!(world.watches_zone_file());
        let calls = world.probe.calls;
        std::fs::write(world.zone.path().join("unrelated"), "x").expect("written");
        world.settle();
        assert_eq!(world.probe.calls, calls, "other files are not the zone");
        world.replace_zone_file();
        world.dispatch_until(|probe| probe.calls > calls);
        assert!(
            world.probe.calls > calls,
            "replacing the zone file is a change"
        );
    }

    #[test]
    fn the_zone_file_is_watched_while_timedated_is_stopped() {
        let mut world = World::new();
        world.start_timedated();
        world.start_clock();
        world.dispatch_until(|probe| {
            probe
                .clock
                .as_ref()
                .is_some_and(|clock| clock.zone_watch.is_none())
        });
        assert!(!world.watches_zone_file(), "timedated runs");
        world.probe.timedated = None;
        world.dispatch_until(|probe| {
            probe
                .clock
                .as_ref()
                .is_some_and(|clock| clock.zone_watch.is_some())
        });
        assert!(world.watches_zone_file(), "timedated stopped");
        world.start_timedated();
        world.dispatch_until(|probe| {
            probe
                .clock
                .as_ref()
                .is_some_and(|clock| clock.zone_watch.is_none())
        });
        assert!(!world.watches_zone_file(), "timedated runs again");
        let calls = world.probe.calls;
        let deadline = Instant::now() + PATIENCE;
        while world.probe.calls == calls && Instant::now() < deadline {
            world.announce_zone();
            world.settle();
        }
        assert!(
            world.probe.calls > calls,
            "a timedated started after the clock is heard, as one the bus starts on demand"
        );
    }

    #[test]
    fn losing_the_bus_falls_back_to_the_zone_file() {
        let mut world = World::new();
        world.start_timedated();
        world.start_clock();
        world.dispatch_until(|probe| {
            probe
                .clock
                .as_ref()
                .is_some_and(|clock| clock.zone_watch.is_none())
        });
        assert!(!world.watches_zone_file());
        world.bus.stop();
        world.dispatch_until(|probe| {
            probe
                .clock
                .as_ref()
                .is_some_and(|clock| clock.zone_watch.is_some())
        });
        assert!(world.watches_zone_file(), "the file stands in for the bus");
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
        let mut world = World::new();
        world.start_clock();
        world.dispatch_until(|probe| {
            probe
                .clock
                .as_ref()
                .is_some_and(|clock| clock.zone_watch.is_some())
        });
        assert_eq!(
            world
                .ledger
                .count(crate::resources::ResourceKind::EventSource)
                .live,
            3,
            "the alarm, timedated's news and the file watch"
        );
        world.probe.clock = None;
        assert_eq!(
            world
                .ledger
                .count(crate::resources::ResourceKind::EventSource)
                .live,
            0
        );
    }
}

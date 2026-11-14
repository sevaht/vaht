//! Event-loop registrations that unregister themselves when dropped.
//!
//! An [`EventSource`] is the only way vaht registers anything with the event
//! loop (clippy's `disallowed_methods` forbids calling `insert_source`
//! elsewhere). Its owner holds it for exactly as long as the source should
//! fire; dropping it removes the source. A window that owns its auto-raise
//! [`Timer`] therefore cannot leave a timer firing after it closes. Each
//! registration is counted in the [`ResourceLedger`].
//!
//! Asynchronous work (a D-Bus signal stream) runs on the event loop too: a
//! [`StreamSource`] polls a stream there and hands each item to its owner's
//! callback, and dropping it drops the stream.

use std::cell::Cell;
use std::fmt;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

use calloop::futures::{Scheduler, executor};
use calloop::timer::{TimeoutAction, Timer as LoopTimer};
use calloop::{InsertError, LoopHandle, RegistrationToken};
use futures_util::{Stream, StreamExt};

use crate::resources::{ResourceKind, ResourceLedger, Tracked};

/// A source registered with the event loop for as long as this value lives.
///
/// Never drop an `EventSource` from inside its own callback: calloop is still
/// dispatching it then, and unregistering it at that moment panics. A
/// callback that wants its source gone records that, and the owner drops the
/// source afterwards ([`Timer::has_fired`] is the pattern).
pub struct EventSource<State: 'static> {
    /// The loop the source is registered with.
    handle: LoopHandle<'static, State>,
    /// Identifies the registration to remove.
    token: RegistrationToken,
    /// Counts the registration while it exists.
    _tracked: Tracked,
}

impl<State: 'static> EventSource<State> {
    /// Registers `source`, calling `callback` for each of its events.
    ///
    /// # Errors
    ///
    /// Returns calloop's error, which hands the source back, if the source
    /// could not be registered.
    pub fn insert<Source, Callback>(
        handle: &LoopHandle<'static, State>,
        ledger: &ResourceLedger,
        source: Source,
        callback: Callback,
    ) -> Result<Self, InsertError<Source>>
    where
        Source: calloop::EventSource + 'static,
        Callback: FnMut(Source::Event, &mut Source::Metadata, &mut State) -> Source::Ret + 'static,
    {
        #[expect(
            clippy::disallowed_methods,
            reason = "EventSource is the one place allowed to register sources"
        )]
        let token = handle.insert_source(source, callback)?;
        Ok(Self {
            handle: handle.clone(),
            token,
            _tracked: ledger.track(ResourceKind::EventSource),
        })
    }
}

impl<State: 'static> Drop for EventSource<State> {
    fn drop(&mut self) {
        // Removing a source that already removed itself (a timer that fired
        // with TimeoutAction::Drop) is a no-op in calloop.
        self.handle.remove(self.token);
    }
}

impl<State: 'static> fmt::Debug for EventSource<State> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventSource")
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

/// A one-shot timeout; dropping it before it fires cancels it.
#[derive(Debug)]
pub struct Timer<State: 'static> {
    /// The registered timer.
    _source: EventSource<State>,
    /// Set once the timer has fired; calloop has removed it by then.
    fired: Rc<Cell<bool>>,
}

impl<State: 'static> Timer<State> {
    /// Calls `callback` once, `delay` from now, unless dropped first.
    ///
    /// # Errors
    ///
    /// Returns an error if the timer could not be registered.
    pub fn once(
        handle: &LoopHandle<'static, State>,
        ledger: &ResourceLedger,
        delay: Duration,
        callback: impl FnOnce(&mut State) + 'static,
    ) -> Result<Self, calloop::Error> {
        let mut callback = Some(callback);
        let fired = Rc::new(Cell::new(false));
        let fired_in_callback = Rc::clone(&fired);
        let source = EventSource::insert(
            handle,
            ledger,
            LoopTimer::from_duration(delay),
            move |_deadline, (), state| {
                fired_in_callback.set(true);
                if let Some(callback) = callback.take() {
                    callback(state);
                }
                TimeoutAction::Drop
            },
        )
        .map_err(|error| error.error)?;
        Ok(Self {
            _source: source,
            fired,
        })
    }

    /// Returns whether the timer has fired; it can then be dropped or
    /// replaced (outside its own callback).
    #[must_use]
    pub fn has_fired(&self) -> bool {
        self.fired.get()
    }
}

/// A timeout that fires again and again until dropped: once after a first
/// delay, then at a steady interval. A held key's repeat is one.
#[derive(Debug)]
pub struct RepeatingTimer<State: 'static> {
    /// The registered timer.
    _source: EventSource<State>,
}

impl<State: 'static> RepeatingTimer<State> {
    /// Calls `callback` `delay` from now, then every `interval`, until the
    /// timer is dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if the timer could not be registered.
    pub fn start(
        handle: &LoopHandle<'static, State>,
        ledger: &ResourceLedger,
        delay: Duration,
        interval: Duration,
        mut callback: impl FnMut(&mut State) + 'static,
    ) -> Result<Self, calloop::Error> {
        let source = EventSource::insert(
            handle,
            ledger,
            LoopTimer::from_duration(delay),
            move |_deadline, (), state| {
                callback(state);
                TimeoutAction::ToDuration(interval)
            },
        )
        .map_err(|error| error.error)?;
        Ok(Self { _source: source })
    }
}

/// A source that may end on its own, as a listener whose peer hung up
/// does: its owner sees that it ended, and drops it (outside its
/// callback).
#[derive(Debug)]
pub struct EndingSource<State: 'static> {
    /// The registered source.
    _source: EventSource<State>,
    /// Set once the source has asked to be removed; calloop has removed
    /// it by then.
    ended: Rc<Cell<bool>>,
}

impl<State: 'static> EndingSource<State> {
    /// Registers `source`, calling `callback` for each of its events.
    ///
    /// # Errors
    ///
    /// Returns an error if the source could not be registered.
    pub fn insert<Source, Callback>(
        handle: &LoopHandle<'static, State>,
        ledger: &ResourceLedger,
        source: Source,
        callback: Callback,
    ) -> Result<Self, calloop::Error>
    where
        Source: calloop::EventSource + 'static,
        Callback: FnMut(Source::Event, &mut Source::Metadata, &mut State) -> Source::Ret + 'static,
    {
        let ended = Rc::new(Cell::new(false));
        let noticing = NoticingEnd {
            inner: source,
            ended: Rc::clone(&ended),
        };
        let source =
            EventSource::insert(handle, ledger, noticing, callback).map_err(|error| error.error)?;
        Ok(Self {
            _source: source,
            ended,
        })
    }

    /// Returns whether the source has ended; it can then be dropped
    /// (outside its own callback).
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.ended.get()
    }
}

/// A source passing everything to `inner`, noting when it asks to be
/// removed.
struct NoticingEnd<Source> {
    /// The source.
    inner: Source,
    /// Set once it asks to be removed.
    ended: Rc<Cell<bool>>,
}

impl<Source: calloop::EventSource> calloop::EventSource for NoticingEnd<Source> {
    type Event = Source::Event;
    type Metadata = Source::Metadata;
    type Ret = Source::Ret;
    type Error = Source::Error;

    const NEEDS_EXTRA_LIFECYCLE_EVENTS: bool = Source::NEEDS_EXTRA_LIFECYCLE_EVENTS;

    fn process_events<Callback>(
        &mut self,
        readiness: calloop::Readiness,
        token: calloop::Token,
        callback: Callback,
    ) -> Result<calloop::PostAction, Self::Error>
    where
        Callback: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let action = self.inner.process_events(readiness, token, callback)?;
        if matches!(action, calloop::PostAction::Remove) {
            self.ended.set(true);
        }
        Ok(action)
    }

    fn register(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        self.inner.register(poll, token_factory)
    }

    fn reregister(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        self.inner.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut calloop::Poll) -> calloop::Result<()> {
        self.inner.unregister(poll)
    }

    fn before_sleep(&mut self) -> calloop::Result<Option<(calloop::Readiness, calloop::Token)>> {
        self.inner.before_sleep()
    }

    fn before_handle_events(&mut self, events: calloop::EventIterator<'_>) {
        self.inner.before_handle_events(events);
    }
}

/// A stream polled on the event loop, each item handed to a callback, until
/// the stream ends or this is dropped.
#[derive(Debug)]
pub struct StreamSource<State: 'static> {
    /// The executor polling the stream, registered with the loop.
    _executor: EventSource<State>,
}

impl<State: 'static> StreamSource<State> {
    /// Polls `stream` on the loop, calling `callback` with each item.
    ///
    /// # Errors
    ///
    /// Returns an error if the executor cannot be created or registered.
    pub fn start<Item: 'static>(
        handle: &LoopHandle<'static, State>,
        ledger: &ResourceLedger,
        stream: impl Stream<Item = Item> + 'static,
        mut callback: impl FnMut(Item, &mut State) + 'static,
    ) -> Result<Self, calloop::Error> {
        // The executor runs futures to one output each, so each future
        // waits for one item and hands the stream back with it, to be
        // scheduled again.
        let (executor, scheduler) = executor::<NextItem<Item>>()?;
        let stream: PendingStream<Item> = Box::pin(stream);
        Self::schedule_next(&scheduler, stream);
        let source = EventSource::insert(
            handle,
            ledger,
            executor,
            move |(item, stream), (), state| {
                if let Some(item) = item {
                    callback(item, state);
                    Self::schedule_next(&scheduler, stream);
                }
            },
        )
        .map_err(|error| error.error)?;
        Ok(Self { _executor: source })
    }

    /// Schedules waiting for `stream`'s next item.
    fn schedule_next<Item: 'static>(
        scheduler: &Scheduler<NextItem<Item>>,
        mut stream: PendingStream<Item>,
    ) {
        // The executor is gone only once the source is dropped, and with
        // it the wish for more items.
        let _ = scheduler.schedule(async move { (stream.next().await, stream) });
    }
}

/// A stream whose items a [`StreamSource`] waits for.
type PendingStream<Item> = Pin<Box<dyn Stream<Item = Item>>>;

/// A stream's next item, if it has not ended, and the stream.
type NextItem<Item> = (Option<Item>, PendingStream<Item>);

#[cfg(test)]
mod tests {
    use calloop::EventLoop;
    use calloop::ping::make_ping;

    use super::*;

    /// What a test's callbacks record.
    type Events = Vec<&'static str>;

    /// How long a test waits for a timer it expects to have fired.
    const TIMER_WAIT: Duration = Duration::from_millis(50);

    #[test]
    fn a_source_fires_while_held_and_is_removed_when_dropped() {
        let mut event_loop: EventLoop<'static, Events> =
            EventLoop::try_new().expect("an event loop");
        let ledger = ResourceLedger::new();
        let (ping, ping_source) = make_ping().expect("a ping source");
        let source = EventSource::insert(
            &event_loop.handle(),
            &ledger,
            ping_source,
            |(), (), events: &mut Events| {
                events.push("pinged");
            },
        )
        .expect("the ping registers");
        let mut events = Events::new();

        ping.ping();
        event_loop
            .dispatch(Duration::ZERO, &mut events)
            .expect("dispatch");
        assert_eq!(events, ["pinged"]);
        assert_eq!(ledger.count(ResourceKind::EventSource).live, 1);

        drop(source);
        ping.ping();
        event_loop
            .dispatch(Duration::ZERO, &mut events)
            .expect("dispatch");
        assert_eq!(events, ["pinged"], "a dropped source no longer fires");
        assert_eq!(ledger.count(ResourceKind::EventSource).live, 0);
    }

    #[test]
    fn an_ending_source_says_when_it_ended() {
        use std::io::Write as _;

        let mut event_loop: EventLoop<'static, Events> =
            EventLoop::try_new().expect("an event loop");
        let ledger = ResourceLedger::new();
        let (mut writer, reader) = std::os::unix::net::UnixStream::pair().expect("a socket pair");
        let generic =
            calloop::generic::Generic::new(reader, calloop::Interest::READ, calloop::Mode::Level);
        let source = EndingSource::insert(
            &event_loop.handle(),
            &ledger,
            generic,
            |_readiness, _reader, events: &mut Events| {
                events.push("hung up");
                Ok(calloop::PostAction::Remove)
            },
        )
        .expect("the source registers");
        let mut events = Events::new();
        event_loop
            .dispatch(Duration::ZERO, &mut events)
            .expect("dispatch");
        assert!(!source.has_ended());

        writer.write_all(b"x").expect("written");
        event_loop
            .dispatch(Duration::ZERO, &mut events)
            .expect("dispatch");
        assert_eq!(events, ["hung up"]);
        assert!(source.has_ended());
        drop(source);
        assert_eq!(ledger.count(ResourceKind::EventSource).live, 0);
    }

    #[test]
    fn a_timer_fires_once() {
        let mut event_loop: EventLoop<'static, Events> =
            EventLoop::try_new().expect("an event loop");
        let ledger = ResourceLedger::new();
        let timer = Timer::once(
            &event_loop.handle(),
            &ledger,
            Duration::from_millis(1),
            |events: &mut Events| {
                events.push("fired");
            },
        )
        .expect("the timer registers");
        let mut events = Events::new();
        assert!(!timer.has_fired());
        event_loop
            .dispatch(TIMER_WAIT, &mut events)
            .expect("dispatch");
        event_loop
            .dispatch(TIMER_WAIT, &mut events)
            .expect("dispatch");
        assert_eq!(events, ["fired"]);
        assert!(timer.has_fired());
        drop(timer);
        assert_eq!(ledger.count(ResourceKind::EventSource).live, 0);
    }

    #[test]
    fn dropping_a_timer_before_it_fires_cancels_it() {
        let mut event_loop: EventLoop<'static, Events> =
            EventLoop::try_new().expect("an event loop");
        let ledger = ResourceLedger::new();
        let timer = Timer::once(
            &event_loop.handle(),
            &ledger,
            Duration::from_millis(5),
            |events: &mut Events| {
                events.push("fired");
            },
        )
        .expect("the timer registers");
        drop(timer);
        let mut events = Events::new();
        event_loop
            .dispatch(TIMER_WAIT, &mut events)
            .expect("dispatch");
        assert_eq!(events, Events::new(), "a cancelled timer never fires");
    }

    #[test]
    fn a_stream_source_hands_over_each_item_until_the_stream_ends() {
        let mut event_loop: EventLoop<'static, Events> =
            EventLoop::try_new().expect("an event loop");
        let ledger = ResourceLedger::new();
        let items = futures_util::stream::iter(["first", "second"]);
        let source = StreamSource::start(
            &event_loop.handle(),
            &ledger,
            items,
            |item, events: &mut Events| events.push(item),
        )
        .expect("the stream registers");
        let mut events = Events::new();
        for _ in 0..3 {
            event_loop
                .dispatch(Duration::ZERO, &mut events)
                .expect("dispatch");
        }
        assert_eq!(events, ["first", "second"]);
        drop(source);
        assert_eq!(ledger.count(ResourceKind::EventSource).live, 0);
    }

    #[test]
    fn a_repeating_timer_fires_until_dropped() {
        let mut event_loop: EventLoop<'static, Events> =
            EventLoop::try_new().expect("an event loop");
        let ledger = ResourceLedger::new();
        let timer = RepeatingTimer::start(
            &event_loop.handle(),
            &ledger,
            Duration::from_millis(1),
            Duration::from_millis(1),
            |events: &mut Events| {
                events.push("fired");
            },
        )
        .expect("the timer registers");
        let mut events = Events::new();
        while events.len() < 3 {
            event_loop
                .dispatch(TIMER_WAIT, &mut events)
                .expect("dispatch");
        }
        drop(timer);
        assert_eq!(ledger.count(ResourceKind::EventSource).live, 0);
        let fired = events.len();
        event_loop
            .dispatch(TIMER_WAIT, &mut events)
            .expect("dispatch");
        assert_eq!(events.len(), fired, "a dropped timer stops firing");
    }
}

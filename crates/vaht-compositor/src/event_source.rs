//! Event-loop registrations that unregister themselves when dropped.
//!
//! An [`EventSource`] is the only way vaht registers anything with the event
//! loop (clippy's `disallowed_methods` forbids calling `insert_source`
//! elsewhere). Its owner holds it for exactly as long as the source should
//! fire; dropping it removes the source. A window that owns its auto-raise
//! [`Timer`] therefore cannot leave a timer firing after it closes. Each
//! registration is counted in the [`ResourceLedger`].

use std::cell::Cell;
use std::fmt;
use std::rc::Rc;
use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer as LoopTimer};
use calloop::{InsertError, LoopHandle, RegistrationToken};

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
}

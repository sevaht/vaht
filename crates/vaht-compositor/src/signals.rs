//! The signals vaht handles (stop, reload, a program it started exited), read
//! as events.
//!
//! The kernel delivers a process-directed signal to any thread that does not
//! block it. The default action of SIGTERM, SIGINT and SIGHUP ends the whole
//! process, and SIGCHLD is discarded by default, so all of them must be
//! blocked in *every* thread, which means blocking them before the first
//! thread starts: threads inherit the mask of their creator. vaht starts
//! threads early (the log writer, the renderer's), so
//! [`ProcessSignals::block`] belongs at the top of `main`, and the compositor
//! then reads the blocked signals as ordinary events.
//!
//! Programs vaht starts do not inherit the blocked mask: the launcher
//! spawns them with an empty one (a binding test checks this).

use calloop::signals::{Signal, Signals};
use smithay::reexports::calloop::LoopHandle;

use crate::Compositor;
use crate::event_source::EventSource;
use crate::resources::ResourceLedger;

/// What a signal asks of vaht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalRequest {
    /// Stop cleanly (SIGTERM, SIGINT).
    Stop,
    /// Read the configuration again (SIGHUP).
    Reload,
    /// A program vaht started has exited and can be reaped (SIGCHLD).
    ReapPrograms,
}

impl SignalRequest {
    /// Returns what `signal` asks for; only the signals [`ProcessSignals`]
    /// blocks ever arrive.
    fn of(signal: Signal) -> Option<Self> {
        if matches!(signal, Signal::SIGTERM | Signal::SIGINT) {
            Some(Self::Stop)
        } else if signal == Signal::SIGHUP {
            Some(Self::Reload)
        } else if signal == Signal::SIGCHLD {
            Some(Self::ReapPrograms)
        } else {
            None
        }
    }
}

/// The signals vaht handles, blocked and waiting to be read as events.
#[derive(Debug)]
pub struct ProcessSignals {
    /// The blocked signals, readable through a signalfd.
    signals: Signals,
}

impl ProcessSignals {
    /// The signals vaht reads as events.
    const SIGNALS: [Signal; 4] = [
        Signal::SIGTERM,
        Signal::SIGINT,
        Signal::SIGHUP,
        Signal::SIGCHLD,
    ];

    /// Blocks the signals in the calling thread, and so in every thread it
    /// starts afterwards.
    ///
    /// Call it before any thread starts: a thread started earlier keeps the
    /// signals unblocked, and a signal it receives ends the process or is
    /// lost.
    ///
    /// # Errors
    ///
    /// Returns an error if the signals cannot be blocked or their signalfd
    /// cannot be created.
    pub fn block() -> Result<Self, calloop::Error> {
        Ok(Self {
            signals: Signals::new(&Self::SIGNALS)?,
        })
    }

    /// Registers the signals with the event loop, which then acts on each
    /// as its [`SignalRequest`] says. A termination request stops vaht
    /// cleanly: clients are told, and the exit resource report is written.
    ///
    /// # Errors
    ///
    /// Returns an error if the event loop refuses the registration.
    pub(crate) fn watch(
        self,
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
    ) -> Result<EventSource<Compositor>, calloop::Error> {
        EventSource::insert(handle, ledger, self.signals, |event, (), compositor| {
            match SignalRequest::of(event.signal()) {
                Some(SignalRequest::Stop) => {
                    tracing::info!(signal = ?event.signal(), "asked to stop");
                    compositor.stop();
                }
                Some(SignalRequest::Reload) => compositor.reload_configuration(),
                Some(SignalRequest::ReapPrograms) => compositor.launcher.reap(),
                None => {}
            }
        })
        .map_err(|error| error.error)
    }
}

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
//! Programs vaht starts do not inherit the blocked mask: the standard
//! library clears it in the child before `exec` (a UI test checks this).

use calloop::signals::{Signal, Signals};

/// What a signal asks of vaht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignalRequest {
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
    pub(crate) fn of(signal: Signal) -> Option<Self> {
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

    /// Returns the event source the compositor registers.
    pub(crate) fn into_source(self) -> Signals {
        self.signals
    }
}

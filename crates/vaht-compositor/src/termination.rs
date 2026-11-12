//! Requests to stop (SIGTERM, SIGINT) as events, so vaht stops cleanly.
//!
//! The kernel delivers a process-directed signal to any thread that does not
//! block it, and the default action of these two ends the whole process. So
//! they must be blocked in *every* thread, which means blocking them before
//! the first thread starts: threads inherit the mask of their creator. vaht
//! starts threads early (the log writer, the renderer's), so
//! [`TerminationSignals::block`] belongs at the top of `main`, and the
//! compositor then reads the blocked signals as ordinary events.

use calloop::signals::{Signal, Signals};

/// SIGTERM and SIGINT, blocked and waiting to be read as events.
#[derive(Debug)]
pub struct TerminationSignals {
    /// The blocked signals, readable through a signalfd.
    signals: Signals,
}

impl TerminationSignals {
    /// The signals that ask vaht to stop.
    const SIGNALS: [Signal; 2] = [Signal::SIGTERM, Signal::SIGINT];

    /// Blocks the termination signals in the calling thread, and so in every
    /// thread it starts afterwards.
    ///
    /// Call it before any thread starts: a thread started earlier keeps the
    /// signals unblocked, and a signal it receives ends the process.
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

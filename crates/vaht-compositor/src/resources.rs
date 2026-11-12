//! Live counts of every kind of resource vaht holds, to make leaks visible.
//!
//! A [`ResourceLedger`] keeps, per [`ResourceKind`], how many resources are
//! alive and the most that ever were. A type that owns a resource embeds the
//! [`Tracked`] its ledger hands out: constructing it counts the resource, and
//! dropping it uncounts it, so the count is right on every path, early
//! returns and panics included. The cost is two relaxed atomic operations per
//! resource, so tracking is always on, in release builds too.
//!
//! [`ProcessUsage`] adds what the kernel says about the process: its open
//! file descriptors by kind, and its resident memory. A count that only grows
//! across a repeated workload is a leak. See
//! `docs/design/03-resources-logging.md` §3.

mod process;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub use process::{DescriptorCensus, DescriptorKind, ProcessUsage, ProcessUsageError};

/// A kind of resource whose live instances are counted.
///
/// Grows as subsystems arrive; each owner of a resource embeds a [`Tracked`]
/// of its kind.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    strum::EnumCount,
    strum::EnumIter,
    strum::IntoStaticStr,
)]
#[strum(serialize_all = "kebab-case")]
pub enum ResourceKind {
    /// A source registered with the event loop (timers included).
    EventSource,
    /// A connected Wayland client.
    Client,
    /// A managed window.
    Window,
    /// A window's frame: its titlebar panel and border buffers.
    Frame,
}

impl ResourceKind {
    /// Returns the kind's position in the ledger's table.
    const fn index(self) -> usize {
        self as usize
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.into())
    }
}

/// How many resources of one kind are alive, and the most there ever were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KindCount {
    /// Resources alive now.
    pub live: usize,
    /// The highest `live` has been.
    pub peak: usize,
}

/// Counts of live resources by kind; cloning shares the same counts.
#[derive(Debug, Clone)]
pub struct ResourceLedger {
    /// One counter per [`ResourceKind`], indexed by [`ResourceKind::index`].
    counters: Arc<[KindCounter; <ResourceKind as strum::EnumCount>::COUNT]>,
}

impl ResourceLedger {
    /// Creates a ledger with every count at zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            counters: Arc::new(std::array::from_fn(|_| KindCounter::default())),
        }
    }

    /// Returns the count for `kind`.
    #[must_use]
    pub fn count(&self, kind: ResourceKind) -> KindCount {
        self.counters[kind.index()].count()
    }

    /// Returns the count for every kind, in declaration order.
    #[must_use]
    pub fn counts(&self) -> Vec<(ResourceKind, KindCount)> {
        <ResourceKind as strum::IntoEnumIterator>::iter()
            .map(|kind| (kind, self.count(kind)))
            .collect()
    }

    /// Counts a new resource of `kind`; it is uncounted when the returned
    /// [`Tracked`] is dropped.
    #[must_use = "the resource is uncounted as soon as the Tracked is dropped"]
    pub fn track(&self, kind: ResourceKind) -> Tracked {
        self.counters[kind.index()].increment();
        Tracked {
            ledger: self.clone(),
            kind,
        }
    }
}

impl Default for ResourceLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ResourceLedger {
    /// Formats one `kind: live (peak N)` line per kind.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (kind, count) in self.counts() {
            writeln!(formatter, "{kind}: {} (peak {})", count.live, count.peak)?;
        }
        Ok(())
    }
}

/// One counted resource; embed it in the type that owns the resource.
#[derive(Debug)]
pub struct Tracked {
    /// The ledger that counted it.
    ledger: ResourceLedger,
    /// What it counts as.
    kind: ResourceKind,
}

impl Tracked {
    /// Returns what kind of resource this is.
    #[must_use]
    pub const fn kind(&self) -> ResourceKind {
        self.kind
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.ledger.counters[self.kind.index()].decrement();
    }
}

/// The live and peak count of one kind.
#[derive(Debug, Default)]
struct KindCounter {
    /// Resources alive now.
    live: AtomicUsize,
    /// The highest `live` has been.
    peak: AtomicUsize,
}

impl KindCounter {
    /// Returns a consistent-enough snapshot; the two loads are not atomic
    /// together, which a diagnostic report can afford.
    fn count(&self) -> KindCount {
        KindCount {
            live: self.live.load(Ordering::Relaxed),
            peak: self.peak.load(Ordering::Relaxed),
        }
    }

    /// Counts one more resource.
    fn increment(&self) {
        let live = self.live.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak.fetch_max(live, Ordering::Relaxed);
    }

    /// Counts one fewer resource.
    fn decrement(&self) {
        self.live.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_counts_while_alive_and_remembers_the_peak() {
        let ledger = ResourceLedger::new();
        let first = ledger.track(ResourceKind::EventSource);
        let second = ledger.track(ResourceKind::EventSource);
        assert_eq!(
            ledger.count(ResourceKind::EventSource),
            KindCount { live: 2, peak: 2 }
        );
        drop(first);
        drop(second);
        assert_eq!(
            ledger.count(ResourceKind::EventSource),
            KindCount { live: 0, peak: 2 }
        );
    }

    #[test]
    fn clones_share_one_set_of_counts() {
        let ledger = ResourceLedger::new();
        let tracked = ledger.clone().track(ResourceKind::EventSource);
        assert_eq!(ledger.count(ResourceKind::EventSource).live, 1);
        assert_eq!(tracked.kind(), ResourceKind::EventSource);
    }

    #[test]
    fn display_lists_every_kind() {
        let ledger = ResourceLedger::new();
        let _tracked = ledger.track(ResourceKind::EventSource);
        assert_eq!(
            ledger.to_string(),
            "event-source: 1 (peak 1)\nclient: 0 (peak 0)\nwindow: 0 (peak 0)\nframe: 0 (peak 0)\n"
        );
    }
}

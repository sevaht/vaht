//! Waiting for files to be written, woken by the writes themselves.
//!
//! [`until_written`] looks, and each time a file in the directory is written
//! looks again, until it finds what it looks for or time runs out. An
//! inotify watch wakes it (General F9): it never looks on a timer.
//! [`wait_for_file`] waits for a file's contents.

use std::os::fd::AsFd as _;
use std::path::Path;
use std::time::{Duration, Instant};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};

/// How long to wait before failing the test.
pub(crate) const PATIENCE: Duration = Duration::from_secs(10);

/// Calls `look` now and after each write to a file in `directory`, until
/// it finds something; `None` if [`PATIENCE`] runs out first.
///
/// # Panics
///
/// Panics if `directory` cannot be watched.
pub(crate) fn until_written<T>(directory: &Path, mut look: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + PATIENCE;
    let writes =
        Inotify::init(InitFlags::IN_CLOEXEC | InitFlags::IN_NONBLOCK).expect("an inotify instance");
    writes
        .add_watch(
            directory,
            AddWatchFlags::IN_MODIFY
                | AddWatchFlags::IN_CLOSE_WRITE
                | AddWatchFlags::IN_CREATE
                | AddWatchFlags::IN_MOVED_TO,
        )
        .expect("the directory can be watched");
    loop {
        // Look after the watch is in place, so no write is missed.
        if let Some(found) = look() {
            return Some(found);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let timeout = PollTimeout::try_from(remaining).unwrap_or(PollTimeout::MAX);
        let mut watched = [PollFd::new(writes.as_fd(), PollFlags::POLLIN)];
        poll(&mut watched, timeout).expect("the watch can be waited on");
        // Which file changed does not matter: `look` looks again.
        let _ = writes.read_events();
    }
}

/// Waits until the file at `path` holds contents `accept` accepts, and
/// returns them.
///
/// # Panics
///
/// Panics if that does not happen in time, or `path` has no directory.
pub fn wait_for_file(path: &Path, accept: impl Fn(&str) -> bool) -> String {
    let directory = path.parent().expect("a file is in a directory");
    until_written(directory, || {
        std::fs::read_to_string(path)
            .ok()
            .filter(|contents| accept(contents))
    })
    .unwrap_or_else(|| panic!("{} never held what was waited for", path.display()))
}

/// Returns the monotonic clock's time now (`CLOCK_MONOTONIC`), which vaht
/// tells presentation times by.
///
/// # Panics
///
/// Panics if the clock cannot be read.
#[must_use]
pub fn monotonic_now() -> Duration {
    let now = nix::time::clock_gettime(nix::time::ClockId::CLOCK_MONOTONIC)
        .expect("the monotonic clock can be read");
    Duration::from(now)
}

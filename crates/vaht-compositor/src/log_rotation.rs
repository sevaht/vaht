//! How big vaht's log file may grow and how many are kept: limits shared
//! between the log writer, which reads them as it writes, and the
//! compositor, which sets them from the settings (`[logging]`) whenever
//! those change.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use crate::config::{FileCount, LoggingSettings};

/// The log file's limits, shared: every copy sees a change.
#[derive(Debug, Clone)]
pub struct LogRotation(Arc<Limits>);

/// The limits themselves.
#[derive(Debug)]
struct Limits {
    /// The size past which the file starts anew, in bytes.
    file_size_bytes: AtomicU64,
    /// How many files are kept, the current one included.
    file_count: AtomicUsize,
    /// After how many seconds without a write an older file is deleted;
    /// 0 keeps them until rotation drops them.
    delete_after_seconds: AtomicU64,
}

impl LogRotation {
    /// The most files there can be: rotation clears older files up to it
    /// when the count is lowered.
    pub const MAX_FILE_COUNT: usize = FileCount::MAX as usize;

    /// Returns limits of `file_size_bytes` per file and `file_count` files,
    /// no file deleted by age.
    #[must_use]
    pub fn new(file_size_bytes: u64, file_count: usize) -> Self {
        Self(Arc::new(Limits {
            file_size_bytes: AtomicU64::new(file_size_bytes),
            file_count: AtomicUsize::new(file_count),
            delete_after_seconds: AtomicU64::new(0),
        }))
    }

    /// Returns the limits `[logging]` in the configuration directory
    /// `config_directory` sets, or the defaults if it sets none or is not
    /// valid: read before vaht logs anything, so the log file opens with
    /// them; what is wrong with the file is reported when the configuration
    /// is loaded.
    #[must_use]
    pub fn configured(config_directory: &Path) -> Self {
        let settings = std::fs::read_to_string(config_directory.join("settings.toml"))
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok())
            .and_then(|mut table| table.remove("logging"))
            .and_then(|logging| logging.try_into::<LoggingSettings>().ok())
            .unwrap_or_default();
        Self::from_settings(settings)
    }

    /// Returns the size past which the file starts anew, in bytes.
    #[must_use]
    pub fn file_size_bytes(&self) -> u64 {
        self.0.file_size_bytes.load(Ordering::Relaxed)
    }

    /// Returns how many files are kept, the current one included.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.0.file_count.load(Ordering::Relaxed)
    }

    /// Returns how long an older file is kept after its last write, if
    /// files are deleted by age.
    #[must_use]
    pub fn delete_after(&self) -> Option<Duration> {
        match self.0.delete_after_seconds.load(Ordering::Relaxed) {
            0 => None,
            seconds => Some(Duration::from_secs(seconds)),
        }
    }

    /// Deletes older files `after` their last write, or keeps them until
    /// rotation drops them (none), for every copy.
    pub fn set_delete_after(&self, after: Option<Duration>) {
        let seconds = after.map_or(0, |after| after.as_secs().max(1));
        self.0
            .delete_after_seconds
            .store(seconds, Ordering::Relaxed);
    }

    /// Makes the limits `file_size_bytes` per file and `file_count` files,
    /// for every copy.
    pub fn set(&self, file_size_bytes: u64, file_count: usize) {
        self.0
            .file_size_bytes
            .store(file_size_bytes, Ordering::Relaxed);
        self.0.file_count.store(file_count, Ordering::Relaxed);
    }

    /// Takes the limits `settings` set, as they change.
    pub(crate) fn follow(&self, settings: LoggingSettings) {
        self.set(settings.file_size_bytes(), settings.file_count());
        self.set_delete_after(settings.delete_after());
    }

    /// Returns the limits `settings` set.
    fn from_settings(settings: LoggingSettings) -> Self {
        let limits = Self::new(settings.file_size_bytes(), settings.file_count());
        limits.set_delete_after(settings.delete_after());
        limits
    }
}

impl PartialEq for LogRotation {
    /// Limits are the same when they are shared: one changes the other.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for LogRotation {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limits_come_from_the_settings_file_or_the_defaults() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let defaults = LogRotation::configured(directory.path());
        assert_eq!(
            (defaults.file_size_bytes(), defaults.file_count()),
            (10 << 20, 5)
        );
        std::fs::write(
            directory.path().join("settings.toml"),
            "theme = \"x\"\n[logging]\nfile_size_mb = 3\nfiles = 2\n",
        )
        .expect("written");
        let set = LogRotation::configured(directory.path());
        assert_eq!((set.file_size_bytes(), set.file_count()), (3 << 20, 2));
    }

    #[test]
    fn a_wrong_logging_section_falls_back_to_the_defaults() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            directory.path().join("settings.toml"),
            "[logging]\nfiles = 0\n",
        )
        .expect("written");
        let limits = LogRotation::configured(directory.path());
        assert_eq!(limits.file_count(), 5);
    }

    #[test]
    fn every_copy_sees_a_change() {
        let limits = LogRotation::new(1, 1);
        let copy = limits.clone();
        limits.follow(LoggingSettings::default());
        assert_eq!((copy.file_size_bytes(), copy.file_count()), (10 << 20, 5));
    }
}

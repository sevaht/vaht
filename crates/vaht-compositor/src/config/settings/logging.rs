//! `[logging]`: how big vaht's log file may grow and how many are kept.
//!
//! vaht starts `vaht.log` anew at every start and whenever it passes the
//! size, keeping the files before it as `vaht.log.1` (the newest), `.2`, …;
//! so the logs take at most the size times the count on disk.

/// How big the log file may grow, and how many are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LoggingSettings {
    /// The size, in megabytes, past which the file starts anew.
    pub(crate) file_size_mb: FileSizeMegabytes,
    /// How many files are kept, the current one included.
    pub(crate) files: FileCount,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            file_size_mb: FileSizeMegabytes(10),
            files: FileCount(5),
        }
    }
}

impl LoggingSettings {
    /// Returns the size past which the file starts anew, in bytes.
    pub(crate) fn file_size_bytes(self) -> u64 {
        u64::from(self.file_size_mb.0) * 1024 * 1024
    }

    /// Returns how many files are kept, the current one included.
    pub(crate) fn file_count(self) -> usize {
        usize::try_from(self.files.0).expect("at most FileCount::MAX")
    }
}

/// A log file's size in megabytes: from 1 to 1024.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "u32")]
pub(crate) struct FileSizeMegabytes(u32);

impl FileSizeMegabytes {
    /// The largest size: a gigabyte per file is plenty.
    const MAX: u32 = 1024;
}

impl TryFrom<u32> for FileSizeMegabytes {
    type Error = String;

    fn try_from(megabytes: u32) -> Result<Self, String> {
        (1..=Self::MAX)
            .contains(&megabytes)
            .then_some(Self(megabytes))
            .ok_or_else(|| format!("file_size_mb is from 1 to {}, not {megabytes}", Self::MAX))
    }
}

/// How many log files are kept: from 1 to 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "u32")]
pub(crate) struct FileCount(u32);

impl FileCount {
    /// The most files kept, which rotation also clears older files up to,
    /// when the count is lowered.
    pub(crate) const MAX: u32 = 100;
}

impl TryFrom<u32> for FileCount {
    type Error = String;

    fn try_from(count: u32) -> Result<Self, String> {
        (1..=Self::MAX)
            .contains(&count)
            .then_some(Self(count))
            .ok_or_else(|| format!("files is from 1 to {}, not {count}", Self::MAX))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Parses a `[logging]` table.
    fn logging(text: &str) -> Result<LoggingSettings, String> {
        toml::from_str(text).map_err(|error| error.message().to_owned())
    }

    #[test]
    fn the_defaults_keep_five_files_of_ten_megabytes() {
        let settings = logging("").expect("valid");
        assert_eq!(settings.file_size_bytes(), 10 * 1024 * 1024);
        assert_eq!(settings.file_count(), 5);
    }

    #[test]
    fn a_size_and_a_count_are_read() {
        let settings = logging("file_size_mb = 2\nfiles = 9\n").expect("valid");
        assert_eq!(settings.file_size_bytes(), 2 * 1024 * 1024);
        assert_eq!(settings.file_count(), 9);
    }

    #[rstest]
    #[case::no_size("file_size_mb = 0\n", "file_size_mb is from 1 to 1024")]
    #[case::too_big("file_size_mb = 5000\n", "file_size_mb is from 1 to 1024")]
    #[case::no_files("files = 0\n", "files is from 1 to 100")]
    #[case::too_many("files = 101\n", "files is from 1 to 100")]
    fn limits_out_of_range_are_refused(#[case] text: &str, #[case] expected: &str) {
        let error = logging(text).expect_err("refused");
        assert!(error.contains(expected), "{error}");
    }
}

//! Diagnostic output: which events are recorded, where they go, and changing
//! that while vaht runs.
//!
//! Events are `tracing` events, filtered by one [`EnvFilter`] that applies to
//! every destination and can be replaced at runtime through [`LogFilter`]
//! (`vaht msg log-filter`). The destinations are:
//!
//! - **a log file** in the state directory, size-limited by a
//!   [`LogRotation`] (`[logging]` in the settings) and rotated at every
//!   start, so `vaht.log` is always
//!   the current session and `vaht.log.1` the previous one. It is written by
//!   a background thread, so vaht never waits on the disk; lines that arrive
//!   faster than the disk takes them are dropped and counted
//!   ([`Logging::dropped_line_count`]), never allowed to stall a frame;
//! - **journald**, whenever its socket exists, with structured fields as
//!   journal fields;
//! - **standard error**, when it is a terminal, or when neither of the others
//!   could be used, so that no error is ever lost without a trace.
//!
//! See `docs/design/03-resources-logging.md` §4. Records from libraries that
//! use the `log` crate arrive as events too, through `tracing-subscriber`'s
//! default `tracing-log` feature.

use std::backtrace::Backtrace;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal as _, Write};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::panic;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use tracing_appender::non_blocking::{ErrorCounter, NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::{SubscriberInitExt as _, TryInitError};
use tracing_subscriber::{EnvFilter, Registry, filter, reload};
use vaht_compositor::LogRotation;

/// The program's name, as journald and the state directory know it.
pub(crate) const PROGRAM_NAME: &str = env!("CARGO_PKG_NAME");

/// The logging options every command accepts.
#[derive(Debug, Clone, Default, clap::Args)]
#[command(next_help_heading = "Logging")]
pub struct LoggingArguments {
    /// Which events to record, as tracing filter directives such as
    /// `info,vaht_compositor::input=trace`. Overrides -v, -q and --debug.
    #[arg(long, env = "VAHT_LOG", value_name = "FILTER")]
    pub log: Option<String>,

    /// Where the log files go [default: `$XDG_STATE_HOME/vaht`].
    #[arg(long, env = "VAHT_LOG_DIRECTORY", value_name = "DIRECTORY")]
    pub log_directory: Option<PathBuf>,

    /// Whether events also go to journald.
    #[arg(long, env = "VAHT_JOURNALD", value_enum, default_value_t)]
    pub journald: JournaldUse,

    /// How much is recorded when --log is not given.
    #[command(flatten)]
    pub verbosity: Verbosity,
}

impl LoggingArguments {
    /// Installs the process-wide diagnostic output these options describe.
    ///
    /// A log file that cannot be opened does not stop vaht: the problem is
    /// reported through the destinations that did open, and vaht runs
    /// without a file.
    ///
    /// # Errors
    ///
    /// Returns an error if the filter directives are invalid, or if
    /// diagnostic output was already installed in this process.
    /// The log file is held to `limits`, which the compositor keeps in line
    /// with the settings as they change ([`Logging::log_rotation`]).
    pub fn install(&self, limits: LogRotation) -> Result<Logging, LoggingError> {
        let directives = self.filter_directives();
        let (filter_layer, filter_handle) = reload::Layer::new(parse_filter(&directives)?);

        let (log_file, file_problem) = match self.log_directory() {
            Some(directory) => match RotatingFile::open_in(&directory, limits.clone()) {
                Ok(log_file) => (Some(log_file), None),
                Err(problem) => (None, Some(problem)),
            },
            None => (None, Some(LoggingError::NoStateDirectory)),
        };
        let (file_layer, file_sink) = match log_file {
            Some((file, path)) => {
                let (writer, guard) = NonBlockingBuilder::default().lossy(true).finish(file);
                let dropped_lines = writer.error_counter();
                let layer = tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false);
                let sink = FileSink {
                    path,
                    limits,
                    dropped_lines,
                    _guard: guard,
                };
                (Some(layer), Some(sink))
            }
            None => (None, None),
        };
        let journald_layer = match self.journald {
            // An absent journald is normal (no systemd), not a problem to report.
            JournaldUse::Auto => tracing_journald::layer()
                .ok()
                .map(|layer| layer.with_syslog_identifier(PROGRAM_NAME.to_owned())),
            JournaldUse::Off => None,
        };
        let has_other_destination = file_layer.is_some() || journald_layer.is_some();
        let terminal_layer = (io::stderr().is_terminal() || !has_other_destination)
            .then(|| tracing_subscriber::fmt::layer().with_writer(io::stderr));

        tracing_subscriber::registry()
            .with(filter_layer)
            .with(file_layer)
            .with(journald_layer)
            .with(terminal_layer)
            .try_init()
            .map_err(LoggingError::AlreadyInstalled)?;
        install_panic_hook();

        if let Some(problem) = file_problem {
            tracing::warn!("running without a log file: {problem}");
        }
        Ok(Logging {
            filter: LogFilter {
                handle: filter_handle,
                startup_directives: directives,
            },
            file: file_sink,
        })
    }

    /// Returns the filter directives in effect at startup.
    fn filter_directives(&self) -> String {
        self.log
            .clone()
            .unwrap_or_else(|| self.verbosity.level().to_string())
    }

    /// Returns the directory for log files, if one can be determined.
    fn log_directory(&self) -> Option<PathBuf> {
        self.log_directory
            .clone()
            .or_else(|| xdg::BaseDirectories::with_prefix(PROGRAM_NAME).get_state_home())
    }
}

/// Whether events go to journald.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum JournaldUse {
    /// Whenever journald's socket exists.
    #[default]
    Auto,
    /// Never; for tests, and for sessions that want only the log file.
    Off,
}

/// How much is recorded when no filter is given; at most one may be chosen.
#[derive(Debug, Clone, Copy, Default, clap::Args)]
#[group(multiple = false)]
pub struct Verbosity {
    /// Records per-window and per-action events too (debug).
    #[arg(short, long)]
    verbose: bool,

    /// Records warnings and errors only.
    #[arg(short, long)]
    quiet: bool,

    /// Records everything, including per-frame detail (trace).
    #[arg(long)]
    debug: bool,
}

impl Verbosity {
    /// Returns the most detailed level recorded.
    #[must_use]
    pub const fn level(self) -> filter::LevelFilter {
        if self.debug {
            filter::LevelFilter::TRACE
        } else if self.verbose {
            filter::LevelFilter::DEBUG
        } else if self.quiet {
            filter::LevelFilter::WARN
        } else {
            filter::LevelFilter::INFO
        }
    }
}

/// A log file that starts anew once it passes its limits' size, keeping
/// the files before it as `vaht.log.1` (the newest), `vaht.log.2`, …, the
/// oldest removed, so the logs never take more than the limits' files.
#[derive(Debug)]
pub struct RotatingFile {
    /// The current file's path; older files append `.1`, `.2`, ….
    path: PathBuf,
    /// The current file.
    file: File,
    /// How many bytes the current file holds.
    written: u64,
    /// How big it may get, and how many files are kept: the settings',
    /// which may change as vaht runs.
    limits: LogRotation,
    /// When older files were last checked for age.
    pruned_at: Instant,
}

impl RotatingFile {
    /// The current log file's name; older files append `.1`, `.2`, ….
    pub const FILE_NAME: &str = "vaht.log";

    /// Opens the log file in `directory`, held to `limits`, starting a new
    /// file for this session if the previous one holds anything.
    ///
    /// The directory and the files are private to the user (modes 0700 and
    /// 0600): logs at debug level contain the commands vaht runs.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created or the file
    /// cannot be opened or rotated.
    pub fn open_in(directory: &Path, limits: LogRotation) -> Result<(Self, PathBuf), LoggingError> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|source| LoggingError::CreateLogDirectory {
                path: directory.to_owned(),
                source,
            })?;
        let path = directory.join(Self::FILE_NAME);
        let open_error = |source| LoggingError::OpenLogFile {
            path: path.clone(),
            source,
        };
        let mut file = Self::open(path.clone(), limits).map_err(open_error)?;
        if file.written > 0 {
            file.rotate().map_err(open_error)?;
        } else {
            file.delete_old();
        }
        Ok((file, path))
    }

    /// Opens the file at `path` to append to, private to the user (mode
    /// 0600), held to `limits`.
    fn open(path: PathBuf, limits: LogRotation) -> io::Result<Self> {
        let file = Self::open_file(&path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path,
            file,
            written,
            limits,
            pruned_at: Instant::now(),
        })
    }

    /// Opens the file at `path` to append to, creating it private to the
    /// user.
    fn open_file(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
    }

    /// Returns the path of the `number`th older file.
    fn older(&self, number: usize) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{number}"));
        PathBuf::from(name)
    }

    /// Moves every file one older, dropping the oldest (and any beyond the
    /// count, which a lowered count leaves), and starts the current file
    /// anew.
    fn rotate(&mut self) -> io::Result<()> {
        let older_count = self.limits.file_count().saturating_sub(1);
        for number in older_count.max(1)..=LogRotation::MAX_FILE_COUNT {
            Self::remove_if_there(&self.older(number))?;
        }
        if older_count == 0 {
            // No older files are kept: the current one starts over.
            self.file.set_len(0)?;
            self.written = 0;
            return Ok(());
        }
        for number in (1..older_count).rev() {
            Self::rename_if_there(&self.older(number), &self.older(number + 1))?;
        }
        self.file.flush()?;
        fs::rename(&self.path, self.older(1))?;
        self.file = Self::open_file(&self.path)?;
        self.written = 0;
        self.delete_old();
        Ok(())
    }

    /// Deletes the older files not written for longer than the limits
    /// allow, if they set an age; the current file is never deleted. A
    /// file that cannot be read or deleted is left for the next time.
    fn delete_old(&mut self) {
        self.pruned_at = Instant::now();
        let Some(after) = self.limits.delete_after() else {
            return;
        };
        let now = SystemTime::now();
        for number in 1..=LogRotation::MAX_FILE_COUNT {
            let older = self.older(number);
            let is_old = fs::metadata(&older)
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|written| now.duration_since(written).is_ok_and(|age| age > after));
            if is_old {
                let _ = fs::remove_file(&older);
            }
        }
    }

    /// Removes the file at `path`, if there is one.
    fn remove_if_there(path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            Ok(()) | Err(_) => Ok(()),
        }
    }

    /// Renames the file at `from` to `to`, if there is one.
    fn rename_if_there(from: &Path, to: &Path) -> io::Result<()> {
        match fs::rename(from, to) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            Ok(()) | Err(_) => Ok(()),
        }
    }
}

impl Write for RotatingFile {
    /// Writes `buffer`, after starting a new file if the current one has
    /// passed the size: a line is never split between files.
    #[expect(
        clippy::renamed_function_params,
        reason = "the standard library names it buf; vaht spells names out (N1)"
    )]
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        /// How often a long session checks its older files' age.
        const AGE_CHECK_INTERVAL: Duration = Duration::from_hours(1);
        if self.written >= self.limits.file_size_bytes() {
            self.rotate()?;
        } else if self.pruned_at.elapsed() >= AGE_CHECK_INTERVAL {
            self.delete_old();
        }
        let count = self.file.write(buffer)?;
        self.written += u64::try_from(count).unwrap_or(u64::MAX);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Installed diagnostic output; keep it alive for as long as vaht runs.
///
/// Dropping it flushes the log file.
#[derive(Debug)]
pub struct Logging {
    /// The filter shared by every destination.
    filter: LogFilter,
    /// The log file, when one could be opened.
    file: Option<FileSink>,
}

impl Logging {
    /// Returns the filter, to change what is recorded while vaht runs.
    #[must_use]
    pub const fn filter(&self) -> &LogFilter {
        &self.filter
    }

    /// Returns the log file's limits, for the compositor to keep in line
    /// with the settings, or `None` without a log file.
    #[must_use]
    pub fn log_rotation(&self) -> Option<LogRotation> {
        self.file.as_ref().map(|file| file.limits.clone())
    }

    /// Returns the current log file's path, if there is one.
    #[must_use]
    pub fn log_file_path(&self) -> Option<&Path> {
        self.file.as_ref().map(|file| file.path.as_path())
    }

    /// Returns how many lines the log file has lost because the disk could
    /// not keep up, or `None` without a log file.
    #[must_use]
    pub fn dropped_line_count(&self) -> Option<usize> {
        self.file
            .as_ref()
            .map(|file| file.dropped_lines.dropped_lines())
    }
}

/// The filter deciding which events are recorded, replaceable at runtime.
#[derive(Debug)]
pub struct LogFilter {
    /// Replaces the filter layer installed in the registry.
    handle: reload::Handle<EnvFilter, Registry>,
    /// What [`LogFilter::reset`] goes back to.
    startup_directives: String,
}

impl LogFilter {
    /// Returns the directives currently in effect.
    #[must_use]
    pub fn directives(&self) -> String {
        self.handle
            .with_current(ToString::to_string)
            .unwrap_or_default()
    }

    /// Replaces the filter with `directives`, such as `vaht_compositor::focus=debug`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directives are invalid; the filter is then
    /// unchanged.
    pub fn set(&self, directives: &str) -> Result<(), LoggingError> {
        self.handle
            .reload(parse_filter(directives)?)
            .map_err(LoggingError::Reload)
    }

    /// Goes back to the filter vaht started with.
    ///
    /// # Errors
    ///
    /// Returns an error only if the subscriber is gone, which happens only
    /// while the process exits.
    pub fn reset(&self) -> Result<(), LoggingError> {
        self.set(&self.startup_directives)
    }
}

/// Why diagnostic output could not be set up as asked.
#[derive(Debug, thiserror::Error)]
pub enum LoggingError {
    /// The filter directives could not be parsed.
    #[error("invalid log filter {directives:?}")]
    InvalidFilter {
        /// The directives as given.
        directives: String,
        /// What was wrong with them.
        source: filter::ParseError,
    },

    /// Neither --log-directory nor a home directory says where logs go.
    #[error("no state directory: set --log-directory or $XDG_STATE_HOME")]
    NoStateDirectory,

    /// The log directory could not be created.
    #[error("could not create log directory {}", .path.display())]
    CreateLogDirectory {
        /// The directory.
        path: PathBuf,
        /// What the operating system reported.
        source: io::Error,
    },

    /// The log file could not be opened or rotated.
    #[error("could not open log file {}", .path.display())]
    OpenLogFile {
        /// The file.
        path: PathBuf,
        /// What the operating system reported.
        source: io::Error,
    },

    /// Diagnostic output had already been installed for this process.
    #[error("diagnostic output was already installed")]
    AlreadyInstalled(#[source] TryInitError),

    /// The filter could not be replaced, because the subscriber is gone.
    #[error("could not replace the log filter")]
    Reload(#[source] reload::Error),
}

/// The log file and what is needed to account for it.
#[derive(Debug)]
struct FileSink {
    /// The current log file.
    path: PathBuf,
    /// Its limits, which the settings keep in line.
    limits: LogRotation,
    /// Lines lost because the writer thread fell behind.
    dropped_lines: ErrorCounter,
    /// Flushes and stops the writer thread when dropped.
    _guard: WorkerGuard,
}

/// Parses filter directives, keeping them in the error if they are invalid.
fn parse_filter(directives: &str) -> Result<EnvFilter, LoggingError> {
    EnvFilter::try_new(directives).map_err(|source| LoggingError::InvalidFilter {
        directives: directives.to_owned(),
        source,
    })
}

/// Records panics in every destination before the default hook reports them.
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |information| {
        tracing::error!(
            backtrace = %Backtrace::force_capture(),
            "{PROGRAM_NAME} panicked: {information}"
        );
        default_hook(information);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The limits vaht has when the settings set none.
    fn defaults() -> LogRotation {
        LogRotation::new(10 * 1024 * 1024, 5)
    }

    #[test]
    fn verbosity_defaults_to_info() {
        assert_eq!(Verbosity::default().level(), filter::LevelFilter::INFO);
    }

    #[test]
    fn each_verbosity_option_selects_its_level() {
        let debug = Verbosity {
            debug: true,
            ..Verbosity::default()
        };
        let verbose = Verbosity {
            verbose: true,
            ..Verbosity::default()
        };
        let quiet = Verbosity {
            quiet: true,
            ..Verbosity::default()
        };
        assert_eq!(debug.level(), filter::LevelFilter::TRACE);
        assert_eq!(verbose.level(), filter::LevelFilter::DEBUG);
        assert_eq!(quiet.level(), filter::LevelFilter::WARN);
    }

    #[test]
    fn explicit_filter_overrides_verbosity() {
        let arguments = LoggingArguments {
            log: Some("warn,vaht=trace".to_owned()),
            verbosity: Verbosity {
                quiet: true,
                ..Verbosity::default()
            },
            ..LoggingArguments::default()
        };
        assert_eq!(arguments.filter_directives(), "warn,vaht=trace");
    }

    #[test]
    fn filter_changes_take_effect_and_reset_restores_the_startup_filter() {
        let (layer, handle) = reload::Layer::new(parse_filter("info").expect("valid"));
        let filter = LogFilter {
            handle,
            startup_directives: "info".to_owned(),
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            assert!(!tracing::enabled!(tracing::Level::DEBUG));
            filter.set("debug").expect("valid directives");
            assert!(tracing::enabled!(tracing::Level::DEBUG));
            assert!(filter.set("vaht=loud").is_err());
            assert_eq!(
                filter.directives(),
                "debug",
                "a rejected filter changes nothing"
            );
            filter.reset().expect("the subscriber is alive");
            assert!(!tracing::enabled!(tracing::Level::DEBUG));
        });
    }

    #[test]
    fn invalid_filter_is_rejected_with_its_text() {
        let error = parse_filter("vaht=loud").expect_err("'loud' is not a level");
        assert!(error.to_string().contains("vaht=loud"));
    }

    /// The names of the files in `directory`, sorted.
    fn file_names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .expect("the test directory exists")
            .map(|entry| {
                entry
                    .expect("readable entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn each_start_begins_a_new_file_and_keeps_the_previous_one() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        for session in ["first", "second"] {
            let (mut file, _) =
                RotatingFile::open_in(directory.path(), defaults()).expect("the log file opens");
            writeln!(file, "{session}").expect("the log file is writable");
        }
        assert_eq!(file_names(directory.path()), ["vaht.log", "vaht.log.1"]);
        let current = fs::read_to_string(directory.path().join("vaht.log")).expect("readable");
        assert_eq!(current, "second\n");
    }

    #[test]
    fn an_empty_previous_file_is_reused_rather_than_rotated() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        drop(RotatingFile::open_in(directory.path(), defaults()).expect("opens"));
        drop(RotatingFile::open_in(directory.path(), defaults()).expect("opens"));
        assert_eq!(file_names(directory.path()), ["vaht.log"]);
    }

    #[test]
    fn rotation_keeps_at_most_file_count_files() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (mut file, _) =
            RotatingFile::open_in(directory.path(), LogRotation::new(16, 3)).expect("opens");
        for line_number in 0..20 {
            writeln!(file, "line {line_number:04} of the log").expect("writable");
        }
        assert_eq!(
            file_names(directory.path()),
            ["vaht.log", "vaht.log.1", "vaht.log.2"]
        );
    }

    #[test]
    fn the_newest_older_file_is_dot_one_and_lines_are_never_split() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (mut file, _) =
            RotatingFile::open_in(directory.path(), LogRotation::new(10, 3)).expect("opens");
        for line in ["first line\n", "second line\n", "third line\n"] {
            file.write_all(line.as_bytes()).expect("writable");
        }
        let read = |name: &str| fs::read_to_string(directory.path().join(name)).expect("readable");
        assert_eq!(read("vaht.log"), "third line\n");
        assert_eq!(read("vaht.log.1"), "second line\n", "the newest older file");
        assert_eq!(read("vaht.log.2"), "first line\n");
    }

    #[test]
    fn with_one_file_kept_the_file_starts_over() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let (mut file, _) =
            RotatingFile::open_in(directory.path(), LogRotation::new(10, 1)).expect("opens");
        for line in ["first line\n", "second line\n"] {
            file.write_all(line.as_bytes()).expect("writable");
        }
        assert_eq!(file_names(directory.path()), ["vaht.log"]);
        let current = fs::read_to_string(directory.path().join("vaht.log")).expect("readable");
        assert_eq!(current, "second line\n");
    }

    #[test]
    fn a_count_lowered_while_running_clears_the_older_files_at_the_next_rotation() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let limits = LogRotation::new(10, 4);
        let (mut file, _) = RotatingFile::open_in(directory.path(), limits.clone()).expect("opens");
        for number in 0..5 {
            writeln!(file, "line {number:04} of it").expect("writable");
        }
        assert_eq!(
            file_names(directory.path()),
            ["vaht.log", "vaht.log.1", "vaht.log.2", "vaht.log.3"]
        );
        limits.set(10, 2);
        writeln!(file, "after the change").expect("writable");
        assert_eq!(file_names(directory.path()), ["vaht.log", "vaht.log.1"]);
    }

    #[test]
    fn older_files_not_written_for_too_long_are_deleted_but_never_the_current_one() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let written = |name: &str, days_ago: u64| {
            let file = File::create(directory.path().join(name)).expect("created");
            let when = SystemTime::now() - Duration::from_hours(days_ago * 24);
            file.set_modified(when).expect("dated");
        };
        // An empty current file: opening it rotates nothing.
        written("vaht.log", 30);
        written("vaht.log.1", 1);
        written("vaht.log.2", 10);
        let limits = defaults();
        limits.set_delete_after(Some(Duration::from_hours(7 * 24)));
        drop(RotatingFile::open_in(directory.path(), limits).expect("opens"));
        assert_eq!(file_names(directory.path()), ["vaht.log", "vaht.log.1"]);
    }

    #[test]
    fn without_an_age_older_files_are_kept() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let file = File::create(directory.path().join("vaht.log.1")).expect("created");
        file.set_modified(SystemTime::UNIX_EPOCH).expect("dated");
        drop(RotatingFile::open_in(directory.path(), defaults()).expect("opens"));
        assert_eq!(file_names(directory.path()), ["vaht.log", "vaht.log.1"]);
    }

    #[test]
    fn log_files_are_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("a temporary directory");
        let log_directory = directory.path().join("logs");
        let (_, path) = RotatingFile::open_in(&log_directory, defaults()).expect("opens");
        let mode = |path: &Path| fs::metadata(path).expect("exists").permissions().mode() & 0o777;
        assert_eq!(mode(&log_directory), 0o700);
        assert_eq!(mode(&path), 0o600);
    }
}

//! Diagnostic output: the options that control it, and installing it.
//!
//! Diagnostics are `tracing` events. The console (standard error) shows
//! warnings and errors unless [`Verbosity`] says otherwise. With `--log-file`,
//! every event at debug level and above is also appended to that file,
//! whatever the console shows.
//!
//! Both outputs prefix each event with the spans it happened inside, such as
//! those `#[tracing::instrument]` creates: `load{name="work"}: reading file`.
//! Records from libraries that use the `log` crate are captured as events
//! too; `try_init` installs that bridge through `tracing-subscriber`'s
//! default `tracing-log` feature, so keep that feature enabled.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::{SubscriberInitExt as _, TryInitError};

/// The logging options every command accepts.
#[derive(Debug, Clone, Default, clap::Args)]
#[command(next_help_heading = "Logging")]
pub struct LoggingArguments {
    /// Appends every diagnostic, at debug level and above, to FILE.
    #[arg(long, value_name = "FILE")]
    pub log_file: Option<PathBuf>,

    /// How much the console shows.
    #[command(flatten)]
    pub verbosity: Verbosity,
}

impl LoggingArguments {
    /// The level recorded in the log file, independent of the console's.
    const LOG_FILE_LEVEL: LevelFilter = LevelFilter::DEBUG;

    /// Installs the process-wide diagnostic output these options describe.
    ///
    /// # Errors
    ///
    /// Returns an error if the log file cannot be opened for appending, or if
    /// diagnostic output was already installed.
    pub fn install(&self) -> Result<(), LoggingError> {
        let console_layer = tracing_subscriber::fmt::layer()
            .with_writer(io::stderr)
            .without_time()
            .with_target(false)
            .with_filter(self.verbosity.console_level());
        let file_layer = match &self.log_file {
            Some(path) => Some(
                tracing_subscriber::fmt::layer()
                    .with_writer(Mutex::new(Self::open_log_file(path)?))
                    .with_ansi(false)
                    .with_filter(Self::LOG_FILE_LEVEL),
            ),
            None => None,
        };
        tracing_subscriber::registry()
            .with(console_layer)
            .with(file_layer)
            .try_init()
            .map_err(LoggingError::AlreadyInstalled)
    }

    /// Opens `path` for appending, creating it if needed.
    fn open_log_file(path: &Path) -> Result<File, LoggingError> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| LoggingError::OpenLogFile {
                path: path.to_owned(),
                source,
            })
    }
}

/// How much diagnostic output the console shows; at most one may be given.
#[derive(Debug, Clone, Copy, Default, clap::Args)]
#[group(multiple = false)]
pub struct Verbosity {
    /// Shows informational messages too.
    #[arg(short, long)]
    verbose: bool,

    /// Shows errors only.
    #[arg(short, long)]
    quiet: bool,

    /// Shows everything, including debugging detail.
    #[arg(long)]
    debug: bool,
}

impl Verbosity {
    /// Returns the most detailed level the console shows.
    #[must_use]
    pub const fn console_level(self) -> LevelFilter {
        if self.debug {
            LevelFilter::DEBUG
        } else if self.verbose {
            LevelFilter::INFO
        } else if self.quiet {
            LevelFilter::ERROR
        } else {
            LevelFilter::WARN
        }
    }
}

/// Why diagnostic output could not be installed.
#[derive(Debug, thiserror::Error)]
pub enum LoggingError {
    /// The log file could not be opened for appending.
    #[error("could not open log file {}", .path.display())]
    OpenLogFile {
        /// The requested log file.
        path: PathBuf,
        /// What the operating system reported.
        source: io::Error,
    },

    /// Diagnostic output had already been installed for this process.
    #[error("diagnostic output was already installed")]
    AlreadyInstalled(#[source] TryInitError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_shows_warnings_by_default() {
        assert_eq!(Verbosity::default().console_level(), LevelFilter::WARN);
    }

    #[test]
    fn each_option_selects_its_level() {
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
        assert_eq!(debug.console_level(), LevelFilter::DEBUG);
        assert_eq!(verbose.console_level(), LevelFilter::INFO);
        assert_eq!(quiet.console_level(), LevelFilter::ERROR);
    }
}

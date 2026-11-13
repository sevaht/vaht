//! The command line, as a typed model.
//!
//! The doc comments on these types become `--help`.

use std::path::PathBuf;

use vaht_compositor::{BackendChoice, RunOptions};

use crate::logging::{LoggingArguments, PROGRAM_NAME};

/// A stacking Wayland compositor descended from Fluxbox.
///
/// Without a subcommand, runs the compositor.
#[derive(Debug, clap::Parser)]
#[command(version)]
pub struct Arguments {
    /// What to do instead of running the compositor.
    #[command(subcommand)]
    pub command: Option<Subcommand>,

    /// Where vaht shows its frames.
    #[arg(long, env = "VAHT_BACKEND", value_enum, default_value_t)]
    pub backend: BackendArgument,

    /// The directory of the configuration files; by default
    /// `$XDG_CONFIG_HOME/vaht`, normally `~/.config/vaht`.
    #[arg(long, env = "VAHT_CONFIG_DIRECTORY", global = true)]
    pub config_directory: Option<PathBuf>,

    /// How much diagnostic output to record, and where.
    #[command(flatten)]
    pub logging: LoggingArguments,
}

impl Arguments {
    /// Returns how the compositor should run.
    #[must_use]
    pub fn run_options(&self) -> RunOptions {
        RunOptions {
            backend: self.backend.into(),
            config_directory: self.config_directory(),
        }
    }

    /// Returns the configuration directory: the one given, or the XDG one.
    /// Without a home directory to put it in, it is a relative `vaht`, which
    /// most likely does not exist, so the built-in configuration applies.
    #[must_use]
    pub fn config_directory(&self) -> PathBuf {
        self.config_directory.clone().unwrap_or_else(|| {
            xdg::BaseDirectories::with_prefix(PROGRAM_NAME)
                .get_config_home()
                .unwrap_or_else(|| PathBuf::from(PROGRAM_NAME))
        })
    }
}

/// The subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum Subcommand {
    /// Checks the configuration files without running vaht, and says what
    /// is wrong with them.
    Check {
        /// Prints vaht's built-in configuration files instead: what applies
        /// when a file does not exist.
        #[arg(long)]
        print_defaults: bool,
    },
}

/// The `--backend` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum BackendArgument {
    /// In a window of the running compositor, when there is one.
    #[default]
    Automatic,
    /// Nowhere: frames are drawn in memory. For tests.
    Headless,
}

impl From<BackendArgument> for BackendChoice {
    fn from(argument: BackendArgument) -> Self {
        match argument {
            BackendArgument::Automatic => Self::Automatic,
            BackendArgument::Headless => Self::Headless,
        }
    }
}

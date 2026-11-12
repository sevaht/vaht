//! The command line, as a typed model.
//!
//! The doc comments on these types become `--help`.

use vaht_compositor::{BackendChoice, RunOptions};

use crate::logging::LoggingArguments;

/// A stacking Wayland compositor descended from Fluxbox.
#[derive(Debug, clap::Parser)]
#[command(version)]
pub struct Arguments {
    /// Where vaht shows its frames.
    #[arg(long, env = "VAHT_BACKEND", value_enum, default_value_t)]
    pub backend: BackendArgument,

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
        }
    }
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

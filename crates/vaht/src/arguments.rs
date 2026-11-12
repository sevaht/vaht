//! The command line, as a typed model.
//!
//! The doc comments on these types become `--help`.

use crate::logging::LoggingArguments;

/// A stacking Wayland compositor descended from Fluxbox.
#[derive(Debug, clap::Parser)]
#[command(version)]
pub struct Arguments {
    /// How much diagnostic output to show, and where.
    #[command(flatten)]
    pub logging: LoggingArguments,
}

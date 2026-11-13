//! `vaht check`: validates the configuration without running vaht.
//!
//! It reads the files exactly as the compositor does, so a file it accepts
//! is one a reload applies. It runs without logging: it writes its findings
//! to the terminal and leaves the compositor's log file alone.

use std::path::Path;
use std::process::ExitCode;

use vaht_compositor::Config;

use crate::terminal;

/// Checks the configuration in `directory`, printing the result.
#[must_use]
pub fn run(directory: &Path) -> ExitCode {
    match Config::load(directory) {
        Ok(_) => {
            terminal::print(&format!(
                "The configuration in {} is valid.\n",
                directory.display()
            ));
            ExitCode::SUCCESS
        }
        Err(error) => {
            terminal::report_error(&anyhow::Error::new(error));
            ExitCode::FAILURE
        }
    }
}

/// Prints every built-in configuration file, each under a header naming it.
#[must_use]
pub fn print_defaults() -> ExitCode {
    for file in Config::FILES {
        terminal::print(&format!("# ===== {} =====\n{}", file.name, file.defaults));
    }
    ExitCode::SUCCESS
}

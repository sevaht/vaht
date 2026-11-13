//! `vaht check`: validates the configuration without running vaht.
//!
//! It reads the files exactly as the compositor does, so a file it accepts
//! is one a reload applies. It runs without logging: it writes its findings
//! to the terminal and leaves the compositor's log file alone.

use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

use vaht_compositor::Config;

/// Checks the configuration in `directory`, printing the result.
#[must_use]
pub fn check(directory: &Path) -> ExitCode {
    match Config::load(directory) {
        Ok(_) => {
            print(&format!(
                "The configuration in {} is valid.\n",
                directory.display()
            ));
            ExitCode::SUCCESS
        }
        Err(error) => {
            report(&anyhow::Error::new(error));
            ExitCode::FAILURE
        }
    }
}

/// Prints every built-in configuration file, each under a header naming it.
#[must_use]
pub fn print_defaults() -> ExitCode {
    for file in Config::FILES {
        print(&format!("# ===== {} =====\n{}", file.name, file.defaults));
    }
    ExitCode::SUCCESS
}

/// Writes `text` to standard output; a closed pipe is not an error.
fn print(text: &str) {
    // Nothing useful can be done if the reader went away.
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

/// Reports `error` and its causes on standard error.
#[expect(
    clippy::print_stderr,
    reason = "vaht check reports to the terminal, not to the log"
)]
fn report(error: &anyhow::Error) {
    eprintln!("{error:#}");
}

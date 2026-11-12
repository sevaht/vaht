//! The `vaht` executable: parses the command line,
//! installs diagnostic output, and runs the application.

use std::process::ExitCode;

use clap::Parser as _;

use vaht::Arguments;

/// Runs the application and converts its outcome to an exit code.
fn main() -> ExitCode {
    let arguments = Arguments::parse();
    // Held until main returns: dropping it flushes the log file.
    let logging = match arguments.logging.install() {
        Ok(logging) => logging,
        Err(error) => {
            report_without_logging(&error.into());
            return ExitCode::FAILURE;
        }
    };
    match vaht::run(&arguments, &logging) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Prints `error` and its causes directly, for failures that happen before
/// logging exists.
#[expect(
    clippy::print_stderr,
    reason = "logging is not installed, so it cannot report this"
)]
fn report_without_logging(error: &anyhow::Error) {
    eprintln!("ERROR: {error:#}");
}

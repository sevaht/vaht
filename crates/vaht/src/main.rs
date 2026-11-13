//! The `vaht` executable: parses the command line,
//! installs diagnostic output, and runs the application.

use std::process::ExitCode;

use clap::Parser as _;

use vaht::{Arguments, Subcommand};

/// Runs the application and converts its outcome to an exit code.
fn main() -> ExitCode {
    let arguments = Arguments::parse();
    // The tools run without the compositor's logging, which would rotate
    // its log file.
    match arguments.command {
        Some(Subcommand::Check {
            print_defaults: true,
        }) => return vaht::check::print_defaults(),
        Some(Subcommand::Check {
            print_defaults: false,
        }) => return vaht::check::run(&arguments.config_directory()),
        None => {}
    }
    // Before any thread starts: logging starts the first one.
    let signals = match vaht_compositor::ProcessSignals::block() {
        Ok(signals) => signals,
        Err(error) => {
            report_without_logging(&anyhow::Error::new(error).context("could not block signals"));
            return ExitCode::FAILURE;
        }
    };
    // Held until main returns: dropping it flushes the log file.
    let logging = match arguments.logging.install() {
        Ok(logging) => logging,
        Err(error) => {
            report_without_logging(&error.into());
            return ExitCode::FAILURE;
        }
    };
    match vaht::run(&arguments, &logging, signals) {
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

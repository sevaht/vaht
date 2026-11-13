//! The `vaht` executable: parses the command line,
//! installs diagnostic output, and runs the application.

use std::process::ExitCode;

use clap::Parser as _;

use vaht::{Arguments, ImportSource, Subcommand};

/// Runs the application and converts its outcome to an exit code.
fn main() -> ExitCode {
    let arguments = Arguments::parse();
    // The tools run without the compositor's logging, which would rotate
    // its log file.
    match &arguments.command {
        Some(Subcommand::Check {
            print_defaults: true,
        }) => return vaht::check::print_defaults(),
        Some(Subcommand::Check {
            print_defaults: false,
        }) => return vaht::check::run(&arguments.config_directory()),
        Some(Subcommand::Import {
            source: ImportSource::Fluxbox { from, to, force },
        }) => {
            let to = to.clone().unwrap_or_else(|| arguments.config_directory());
            return vaht::import::fluxbox(from.as_deref(), &to, *force);
        }
        None => {}
    }
    // Before any thread starts: logging starts the first one.
    let signals = match vaht_compositor::ProcessSignals::block() {
        Ok(signals) => signals,
        Err(error) => {
            vaht::terminal::report_error(
                &anyhow::Error::new(error).context("could not block signals"),
            );
            return ExitCode::FAILURE;
        }
    };
    // Held until main returns: dropping it flushes the log file.
    let logging = match arguments.logging.install() {
        Ok(logging) => logging,
        Err(error) => {
            vaht::terminal::report_error(&error.into());
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

//! The `vaht` executable: parses the command line,
//! installs diagnostic output, and runs the application.

use std::process::ExitCode;

use clap::Parser as _;

use vaht::{Arguments, ConfigCommand, ImportSource, LiveCommand, Subcommand};

/// Runs `vaht config`: what needs no running vaht here, the rest by asking
/// the running one.
fn run_config(command: &ConfigCommand, arguments: &Arguments) -> ExitCode {
    match command {
        ConfigCommand::Check {
            print_defaults: true,
        } => vaht::config::print_defaults(),
        ConfigCommand::Check {
            print_defaults: false,
        } => vaht::config::check(&arguments.config_directory()),
        ConfigCommand::Schema { file } => vaht::config::schema(file.file()),
        ConfigCommand::Files
        | ConfigCommand::Get { .. }
        | ConfigCommand::Set { .. }
        | ConfigCommand::Unset { .. }
        | ConfigCommand::Save
        | ConfigCommand::Revert { .. } => {
            command.to_request().map_or(ExitCode::FAILURE, |request| {
                vaht::message::send(&request, arguments.json)
            })
        }
    }
}

/// Runs a command that asks the running vaht, printing its reply as JSON
/// when `json`.
fn run_live(command: &LiveCommand, json: bool) -> ExitCode {
    match command {
        LiveCommand::Subscribe { events } => {
            vaht::message::follow(events.iter().map(|list| list.kind()).collect(), json)
        }
        LiveCommand::Pick { title } => vaht::message::pick(title.clone(), json),
        LiveCommand::Version
        | LiveCommand::Run { .. }
        | LiveCommand::Windows
        | LiveCommand::Workspaces
        | LiveCommand::Outputs
        | LiveCommand::LogFilter { .. } => vaht::message::send(&command.to_request(), json),
    }
}

/// Runs the application and converts its outcome to an exit code.
fn main() -> ExitCode {
    let arguments = Arguments::parse();
    // The tools run without the compositor's logging, which would rotate
    // its log file.
    match &arguments.command {
        Some(Subcommand::Config { command }) => return run_config(command, &arguments),
        Some(Subcommand::Import {
            source: ImportSource::Fluxbox { from, to, force },
        }) => {
            let to = to.clone().unwrap_or_else(|| arguments.config_directory());
            return vaht::import::fluxbox(from.as_deref(), &to, *force);
        }
        Some(Subcommand::Import {
            source:
                ImportSource::FluxboxStyle {
                    paths,
                    to,
                    force,
                    notes,
                    separate,
                    options,
                },
        }) => {
            let to = to
                .clone()
                .unwrap_or_else(|| arguments.config_directory().join("styles"));
            let folding = vaht::import::Folding {
                is_separate: *separate,
                option_names: options,
            };
            return vaht::import::fluxbox_styles(paths, &to, (*force, *notes), &folding);
        }
        Some(Subcommand::IpcSchema) => return vaht::message::print_schema(),
        Some(Subcommand::Live(command)) => return run_live(command, arguments.json),
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
    // The log file opens with the settings' limits ([logging]).
    let limits = vaht_compositor::LogRotation::configured(&arguments.config_directory());
    let logging = match arguments.logging.install(limits) {
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

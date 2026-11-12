//! The `vaht` command: one executable for the compositor and its tools.
//!
//! Its subcommands (running the compositor, `msg`, `dialog`, `import`,
//! `check`) each hand off to the library crate that implements them; see
//! `docs/design/01-object-model.md` §2. Everything the binary does lives in
//! this library, and `src/main.rs` only connects it to the process, so all of
//! it is reachable from tests and from the generated documentation.

pub mod arguments;
pub mod logging;

pub use arguments::Arguments;
pub use logging::Logging;

/// Runs the command line's request, with diagnostic output already installed.
///
/// # Errors
///
/// Returns an error describing why the request could not finish.
pub fn run(
    arguments: &Arguments,
    logging: &Logging,
    termination: vaht_compositor::TerminationSignals,
) -> anyhow::Result<()> {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        log_file = ?logging.log_file_path(),
        filter = %logging.filter().directives(),
        "vaht starting"
    );
    tracing::debug!(?arguments, "command line");
    vaht_compositor::Compositor::run(arguments.run_options(), termination)?;
    Ok(())
}

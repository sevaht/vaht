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

/// Runs the application with the parsed command line.
///
/// # Errors
///
/// Returns an error describing why the application could not finish.
pub fn run(arguments: &Arguments) -> anyhow::Result<()> {
    tracing::debug!(?arguments, "starting");
    tracing::warn!("application code goes here!");
    Ok(())
}

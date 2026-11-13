//! Writing to the terminal: what the tools (`vaht check`, `vaht import`)
//! print, and failures that happen while the compositor's logging is not
//! running.
//!
//! The tools run without logging, so they leave the compositor's log file
//! alone; they report here instead.

use std::io::Write as _;

/// Writes `text` to standard output; a closed pipe is not an error.
pub fn print(text: &str) {
    // Nothing useful can be done if the reader went away.
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

/// Reports `error` and its causes on standard error.
#[expect(
    clippy::print_stderr,
    reason = "these failures happen where no logging is installed"
)]
pub fn report_error(error: &anyhow::Error) {
    eprintln!("error: {error:#}");
}

//! `vaht import`: converts another window manager's configuration.
//!
//! It runs without logging, writing to the terminal: the files written,
//! then every line not converted and why.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vaht_import::FluxboxSetup;

/// Converts the Fluxbox setup in `from` (or `~/.fluxbox`) into `to`,
/// replacing existing files only with `replace`.
#[must_use]
pub fn fluxbox(from: Option<&Path>, to: &Path, replace: bool) -> ExitCode {
    let from = from.map_or_else(default_fluxbox_directory, Path::to_path_buf);
    let conversion = match FluxboxSetup::read(&from) {
        Ok(setup) => setup.convert(),
        Err(error) => {
            report(&anyhow::Error::new(error));
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = conversion.write(to, replace) {
        report(&anyhow::Error::new(error));
        return ExitCode::FAILURE;
    }
    let mut output = std::io::stdout().lock();
    // Nothing useful can be done if the reader went away.
    for file in conversion.files() {
        let _ = writeln!(output, "Wrote {}", to.join(file.name).display());
    }
    let notes = conversion.report().notes().len();
    if notes > 0 {
        let _ = write!(
            output,
            "\nNot converted ({notes}):\n{}",
            conversion.report()
        );
    }
    ExitCode::SUCCESS
}

/// Returns `~/.fluxbox`, Fluxbox's own directory.
fn default_fluxbox_directory() -> PathBuf {
    std::env::var_os("HOME").map_or_else(
        || PathBuf::from(".fluxbox"),
        |home| Path::new(&home).join(".fluxbox"),
    )
}

/// Reports `error` and its causes on standard error.
#[expect(
    clippy::print_stderr,
    reason = "vaht import reports to the terminal, not to the log"
)]
fn report(error: &anyhow::Error) {
    eprintln!("{error:#}");
}

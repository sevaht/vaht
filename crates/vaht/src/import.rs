//! `vaht import`: converts another window manager's configuration.
//!
//! It runs without logging, writing to the terminal: the files written,
//! then every line not converted and why.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vaht_import::{FluxboxSetup, ImportError};

use crate::terminal;

/// Converts the Fluxbox setup in `from` (or `~/.fluxbox`) into `to`,
/// replacing existing files only with `replace`.
#[must_use]
pub fn fluxbox(from: Option<&Path>, to: &Path, replace: bool) -> ExitCode {
    let from = from.map_or_else(default_fluxbox_directory, Path::to_path_buf);
    let conversion = match FluxboxSetup::read(&from) {
        Ok(setup) => setup.convert(),
        Err(error) => {
            terminal::report_error(&anyhow::Error::new(error));
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = conversion.write(to, replace) {
        terminal::report_error(&anyhow::Error::new(error));
        return ExitCode::FAILURE;
    }
    let mut output = String::new();
    for file in conversion.files() {
        writeln!(output, "Wrote {}", to.join(&file.name).display())
            .expect("writing to a String cannot fail");
    }
    let notes = conversion.report().notes().len();
    if notes > 0 {
        write!(
            output,
            "\nNot converted ({notes}):\n{}",
            conversion.report()
        )
        .expect("writing to a String cannot fail");
    }
    terminal::print(&output);
    ExitCode::SUCCESS
}

/// What happened to one style.
enum Outcome {
    /// Written, with this many lines not converted.
    Written(usize),
    /// Left alone: the file exists and `--force` was not given.
    Left,
    /// Failed, or its name was already taken by a style of this run.
    Failed,
}

/// Converts the Fluxbox styles at `paths` (each a style, or a directory of
/// them) into the directory `to`, one `<name>.toml` each. `replace` replaces
/// styles that exist; `all_notes` prints every line not converted (one style
/// prints them anyway).
///
/// A style that fails does not stop the others. The exit code is a failure if
/// any failed, or if the one style asked for was left alone.
#[must_use]
pub fn fluxbox_styles(
    paths: &[PathBuf],
    to: &Path,
    (replace, all_notes): (bool, bool),
) -> ExitCode {
    let mut styles = Vec::new();
    for path in paths {
        match vaht_import::find_styles(path) {
            Ok(found) if found.is_empty() => {
                terminal::report_error(&anyhow::anyhow!(
                    "no Fluxbox styles found in {}",
                    path.display()
                ));
                return ExitCode::FAILURE;
            }
            Ok(found) => styles.extend(found),
            Err(error) => {
                terminal::report_error(&anyhow::Error::new(error));
                return ExitCode::FAILURE;
            }
        }
    }
    let show_notes = all_notes || styles.len() == 1;
    let mut taken = HashSet::new();
    let mut outcomes = Vec::new();
    for style in &styles {
        let outcome = convert_one(style, to, (replace, show_notes), &mut taken);
        outcomes.push(outcome);
    }
    let written = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, Outcome::Written(_)))
        .count();
    let left = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, Outcome::Left))
        .count();
    let failed = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, Outcome::Failed))
        .count();
    if styles.len() > 1 {
        let notes: usize = outcomes
            .iter()
            .map(|outcome| match outcome {
                Outcome::Written(notes) => *notes,
                Outcome::Left | Outcome::Failed => 0,
            })
            .sum();
        terminal::print(&format!(
            "\nConverted {written} of {} styles into {} ({left} left alone, {failed} failed); \
             {notes} lines not converted{}.\n",
            styles.len(),
            to.display(),
            if all_notes {
                ""
            } else {
                " (--notes lists them)"
            },
        ));
    }
    if failed > 0 || (styles.len() == 1 && left == 1) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Converts and writes the style at `path`, printing what happened.
fn convert_one(
    path: &Path,
    to: &Path,
    (replace, show_notes): (bool, bool),
    taken: &mut HashSet<String>,
) -> Outcome {
    let converted = match vaht_import::convert_style(path) {
        Ok(converted) => converted,
        Err(error) => {
            terminal::report_error(&anyhow::Error::new(error).context(path.display().to_string()));
            return Outcome::Failed;
        }
    };
    if !taken.insert(converted.name().to_owned()) {
        terminal::report_error(&anyhow::anyhow!(
            "{}: its name, {}, is taken by another style of this run; convert them separately \
             or into other directories",
            path.display(),
            converted.name()
        ));
        return Outcome::Failed;
    }
    match converted.write(to, replace) {
        Ok(written) => {
            let notes = converted.report().notes().len();
            let mut output = format!("Wrote {}", written.display());
            if notes > 0 {
                write!(output, " ({notes} lines not converted)")
                    .expect("writing to a String cannot fail");
            }
            output.push('\n');
            if show_notes && notes > 0 {
                write!(output, "\n{}\n", converted.report())
                    .expect("writing to a String cannot fail");
            }
            terminal::print(&output);
            Outcome::Written(notes)
        }
        Err(ImportError::Exists(existing)) => {
            terminal::report_error(&anyhow::anyhow!(
                "{} exists; pass --force to replace it",
                existing.display()
            ));
            Outcome::Left
        }
        Err(error) => {
            terminal::report_error(&anyhow::Error::new(error).context(path.display().to_string()));
            Outcome::Failed
        }
    }
}

/// Returns `~/.fluxbox`, Fluxbox's own directory.
fn default_fluxbox_directory() -> PathBuf {
    std::env::var_os("HOME").map_or_else(
        || PathBuf::from(".fluxbox"),
        |home| Path::new(&home).join(".fluxbox"),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn imported_rules_for_the_run_window_match_the_run_dialog() {
        assert_eq!(
            vaht_import::RUN_DIALOG_APP_ID,
            vaht_dialog::RunDialog::APP_ID
        );
    }
}

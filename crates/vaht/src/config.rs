//! The parts of `vaht config` that need no running vaht: `check`, `check
//! --print-defaults` and `schema`.
//!
//! `check` reads the files exactly as the compositor does, and lists every
//! problem the compositor would leave out. These run without logging: they
//! write to the terminal and leave the compositor's log file alone.

use std::path::Path;
use std::process::ExitCode;

use vaht_compositor::Config;

use crate::terminal;

/// Checks the configuration in `directory`, printing the result.
#[must_use]
pub fn check(directory: &Path) -> ExitCode {
    let problems = Config::problems(directory);
    if problems.is_empty() {
        terminal::print(&format!(
            "The configuration in {} is valid.\n",
            directory.display()
        ));
        return ExitCode::SUCCESS;
    }
    // Every one, not the first: a running vaht uses what it can and shows all
    // that it left out.
    for problem in problems {
        terminal::report_error(&anyhow::anyhow!(problem));
    }
    ExitCode::FAILURE
}

/// Prints every built-in configuration file, each under a header naming it,
/// then the built-in theme, to copy into `themes/<name>.toml`.
#[must_use]
pub fn print_defaults() -> ExitCode {
    for file in Config::FILES {
        terminal::print(&format!("# ===== {} =====\n{}", file.name, file.defaults));
    }
    terminal::print(&format!(
        "# ===== themes/<name>.toml: the built-in theme =====\n{}",
        vaht_ui::Theme::BUILT_IN
    ));
    ExitCode::SUCCESS
}

/// Prints the JSON Schema of `file`: every key with its type, documentation,
/// default and range. It is generated from vaht's code, so it needs no
/// running vaht.
///
/// # Panics
///
/// Never: a schema is plain data that always encodes.
#[must_use]
pub fn schema(file: vaht_ipc::ConfigFile) -> ExitCode {
    let Some(schema) = Config::schema(file) else {
        terminal::report_error(&anyhow::anyhow!("{} has no schema yet", file.file_name()));
        return ExitCode::FAILURE;
    };
    let mut text =
        serde_json::to_string_pretty(&schema).expect("a schema is plain data, always encodable");
    text.push('\n');
    terminal::print(&text);
    ExitCode::SUCCESS
}

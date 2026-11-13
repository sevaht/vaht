//! `vaht dialog`: one of vaht's dialogs, in the configured theme.

use std::path::Path;
use std::process::ExitCode;

use vaht_compositor::Config;
use vaht_dialog::{LineDialog, RunDialog};
use vaht_ui::Theme;

use crate::DialogKind;
use crate::terminal;

/// Shows the dialog `kind`, styled by the theme the configuration in
/// `config_directory` names, until it closes.
#[must_use]
pub fn show(kind: DialogKind, config_directory: &Path) -> ExitCode {
    let theme = match Config::load(config_directory) {
        Ok(config) => config.theme().clone(),
        Err(error) => {
            terminal::report_error(
                &anyhow::Error::new(error)
                    .context("the configuration is not valid; using the built-in theme"),
            );
            Theme::built_in()
        }
    };
    let dialog: Box<dyn LineDialog> = match kind {
        DialogKind::Run => Box::new(RunDialog::new()),
    };
    match vaht_dialog::show(dialog, theme) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            terminal::report_error(&anyhow::Error::new(error));
            ExitCode::FAILURE
        }
    }
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

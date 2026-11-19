//! `vaht-dialog`: one of vaht's dialogs (`run`, `command`, or `title
//! --window <id>`), a window with a line to type in. vaht starts it for the
//! `run-dialog`, `command-dialog` and `set-title-dialog` actions.

use vaht_dialog::{CommandDialog, LineDialog, RunDialog, TitleDialog};
use vaht_ipc::{Client, Reply, Request};
use vaht_settings::dialog::DialogApp;

fn main() -> anyhow::Result<()> {
    DialogApp::run(dialog()?)
}

/// Returns the dialog the command line asks for.
fn dialog() -> anyhow::Result<Box<dyn LineDialog>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["run"] => Ok(Box::new(RunDialog::new())),
        ["command"] => Ok(Box::new(CommandDialog::new())),
        ["title", "--window", id] => {
            let window: u64 = id
                .parse()
                .map_err(|error| anyhow::anyhow!("`{id}` is not a window id: {error}"))?;
            Ok(Box::new(TitleDialog::new(window, &title_of(window)?)))
        }
        _ => anyhow::bail!("usage: vaht-dialog run | command | title --window <id>"),
    }
}

/// Returns the title of the window `window` (by id), asking vaht.
fn title_of(window: u64) -> anyhow::Result<String> {
    let reply = Client::connect_from_environment()?.request(&Request::Windows {})?;
    let Reply::Windows { windows } = reply else {
        anyhow::bail!("vaht did not list its windows: {reply:?}");
    };
    windows
        .into_iter()
        .find(|listed| listed.id == window)
        .map(|listed| listed.title)
        .ok_or_else(|| anyhow::anyhow!("there is no window {window}"))
}

//! `vaht msg`: one request to the running vaht, and its reply printed.

use std::fmt::Write as _;
use std::process::ExitCode;

use vaht_ipc::{Client, Reply, Request};

use crate::terminal;

/// Sends `request` to the vaht this runs under and prints the reply: as
/// its JSON when `json`, else for a person to read.
///
/// # Panics
///
/// Never: replies are plain data that always encode.
#[must_use]
pub fn send(request: &Request, json: bool) -> ExitCode {
    let reply = Client::connect_from_environment().and_then(|mut client| client.request(request));
    match reply {
        Ok(Reply::Error { message }) if !json => {
            terminal::report_error(&anyhow::anyhow!(message));
            ExitCode::FAILURE
        }
        Ok(reply) => {
            let failed = matches!(reply, Reply::Error { .. });
            terminal::print(&if json {
                let mut line = serde_json::to_string(&reply)
                    .expect("replies are plain data, always encodable");
                line.push('\n');
                line
            } else {
                readable(&reply)
            });
            if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            terminal::report_error(&anyhow::Error::new(error));
            ExitCode::FAILURE
        }
    }
}

/// Returns `reply` as lines for a person to read.
fn readable(reply: &Reply) -> String {
    let mut text = String::new();
    match reply {
        Reply::Version { version } => {
            let _ = writeln!(text, "{version}");
        }
        Reply::Done {} => {}
        Reply::Windows { windows } => {
            for window in windows {
                let workspace = window
                    .workspace
                    .map_or_else(|| "*".to_owned(), |number| number.to_string());
                let state = [
                    (window.is_focused, 'f'),
                    (window.is_minimized, 'm'),
                    (window.is_shaded, 's'),
                ]
                .iter()
                .map(|(is_set, letter)| if *is_set { *letter } else { '-' })
                .collect::<String>();
                let _ = writeln!(
                    text,
                    "{}\t{workspace}\t{state}\t{}\t{}",
                    window.id, window.app_id, window.title
                );
            }
        }
        Reply::Workspaces { workspaces } => {
            for workspace in workspaces {
                let mark = if workspace.is_current { '*' } else { ' ' };
                let _ = writeln!(text, "{mark} {}\t{}", workspace.number, workspace.name);
            }
        }
        Reply::Error { message } => {
            let _ = writeln!(text, "error: {message}");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use vaht_ipc::{WindowInfo, WorkspaceInfo};

    use super::*;

    #[test]
    fn windows_print_a_line_each() {
        let reply = Reply::Windows {
            windows: vec![WindowInfo {
                id: 7,
                title: "~/scm".to_owned(),
                app_id: "kitty".to_owned(),
                workspace: None,
                is_focused: true,
                is_minimized: false,
                is_shaded: true,
            }],
        };
        assert_eq!(readable(&reply), "7\t*\tf-s\tkitty\t~/scm\n");
    }

    #[test]
    fn the_shown_workspace_is_marked() {
        let reply = Reply::Workspaces {
            workspaces: vec![
                WorkspaceInfo {
                    number: 1,
                    name: "one".to_owned(),
                    is_current: false,
                },
                WorkspaceInfo {
                    number: 2,
                    name: "two".to_owned(),
                    is_current: true,
                },
            ],
        };
        assert_eq!(readable(&reply), "  1\tone\n* 2\ttwo\n");
    }
}

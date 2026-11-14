//! `vaht msg`: one request to the running vaht, and its reply printed.

use std::fmt::Write as _;
use std::process::ExitCode;

use vaht_ipc::{Client, ClientError, EventKind, OutputInfo, Reply, Request};

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

/// Subscribes to the lists `events` and prints each as vaht sends it, as
/// JSON when `json`, else for a person to read (a blank line after each),
/// until vaht stops.
///
/// # Panics
///
/// Never: events are plain data that always encode.
#[must_use]
pub fn follow(events: Vec<EventKind>, json: bool) -> ExitCode {
    let mut client = match Client::connect_from_environment()
        .and_then(|mut client| client.subscribe(events).map(|()| client))
    {
        Ok(client) => client,
        Err(error) => {
            terminal::report_error(&anyhow::Error::new(error));
            return ExitCode::FAILURE;
        }
    };
    loop {
        match client.next_event() {
            Ok(event) => terminal::print(&if json {
                let mut line =
                    serde_json::to_string(&event).expect("events are plain data, always encodable");
                line.push('\n');
                line
            } else {
                format!("{}\n", readable(&event.into_reply()))
            }),
            Err(ClientError::Closed) => return ExitCode::SUCCESS,
            Err(error) => {
                terminal::report_error(&anyhow::Error::new(error));
                return ExitCode::FAILURE;
            }
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
        Reply::Outputs { outputs, profile } => {
            if let Some(profile) = profile {
                let _ = writeln!(text, "profile {profile}");
            }
            for output in outputs {
                output_lines(&mut text, output);
            }
        }
        Reply::Error { message } => {
            let _ = writeln!(text, "error: {message}");
        }
    }
    text
}

/// Appends `output`'s lines to `text`: a line about it, then each `match`
/// that finds it, ready to paste into `outputs.toml`.
fn output_lines(text: &mut String, output: &OutputInfo) {
    let mark = if !output.is_enabled {
        '-'
    } else if output.is_primary {
        '*'
    } else {
        ' '
    };
    let state = output.mode.map_or_else(
        || "off".to_owned(),
        |mode| {
            format!(
                "{}x{}@{}\t{},{}\tscale {}",
                mode.width,
                mode.height,
                f64::from(mode.refresh_millihertz) / 1000.0,
                output.position[0],
                output.position[1],
                output.scale
            )
        },
    );
    let display = format!("{} {}", output.make, output.model);
    let _ = writeln!(
        text,
        "{}",
        format!("{mark} {}\t{state}\t{}", output.connector, display.trim()).trim_end()
    );
    let _ = writeln!(text, "    match = {{ connector = {:?} }}", output.connector);
    let display: Vec<String> = [
        ("make", &output.make),
        ("model", &output.model),
        ("serial", &output.serial),
    ]
    .iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(key, value)| format!("{key} = {value:?}"))
    .collect();
    if !display.is_empty() {
        let _ = writeln!(text, "    match = {{ {} }}", display.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use vaht_ipc::{ModeInfo, WindowInfo, WorkspaceInfo};

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

    #[test]
    fn outputs_print_with_matches_to_paste() {
        let reply = Reply::Outputs {
            outputs: vec![
                OutputInfo {
                    connector: "HDMI-A-1".to_owned(),
                    make: "Acer".to_owned(),
                    model: "XB271HU".to_owned(),
                    serial: String::new(),
                    is_enabled: true,
                    is_primary: true,
                    mode: Some(ModeInfo {
                        width: 1920,
                        height: 1080,
                        refresh_millihertz: 60_000,
                    }),
                    position: [0, 0],
                    scale: 1.0,
                },
                OutputInfo {
                    connector: "eDP-1".to_owned(),
                    make: String::new(),
                    model: String::new(),
                    serial: String::new(),
                    is_enabled: false,
                    is_primary: false,
                    mode: None,
                    position: [0, 0],
                    scale: 1.0,
                },
            ],
            profile: Some("desk".to_owned()),
        };
        assert_eq!(
            readable(&reply),
            "profile desk\n\
             * HDMI-A-1\t1920x1080@60\t0,0\tscale 1\tAcer XB271HU\n    \
             match = { connector = \"HDMI-A-1\" }\n    \
             match = { make = \"Acer\", model = \"XB271HU\" }\n\
             - eDP-1\toff\n    \
             match = { connector = \"eDP-1\" }\n"
        );
    }
}

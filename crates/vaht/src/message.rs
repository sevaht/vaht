//! The commands that ask the running vaht: one request, and its reply printed.

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
            // A pick closed without a choice fails too, as dmenu does.
            let failed = matches!(reply, Reply::Error { .. } | Reply::Picked { choice: None });
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

/// Reads lines from standard input and asks vaht to show them as a menu
/// titled `title`, printing the line the user chooses (as JSON when
/// `json`): a dmenu-style chooser. Empty lines are left out. Fails,
/// printing nothing, if the user closed the menu, or there was nothing to
/// pick.
#[must_use]
pub fn pick(title: Option<String>, json: bool) -> ExitCode {
    let items: Vec<String> = match std::io::read_to_string(std::io::stdin()) {
        Ok(text) => text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect(),
        Err(error) => {
            terminal::report_error(&anyhow::Error::new(error).context("could not read the items"));
            return ExitCode::FAILURE;
        }
    };
    send(&Request::Pick { title, items }, json)
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

/// Prints the IPC protocol's JSON Schema.
///
/// # Panics
///
/// Never: the schema is plain data that always encodes.
#[must_use]
pub fn print_schema() -> ExitCode {
    let mut text = serde_json::to_string_pretty(&vaht_ipc::schema())
        .expect("the schema is plain data, always encodable");
    text.push('\n');
    terminal::print(&text);
    ExitCode::SUCCESS
}

/// Returns `reply` as lines for a person to read.
fn readable(reply: &Reply) -> String {
    let mut text = String::new();
    match reply {
        Reply::Version { version } => {
            let _ = writeln!(text, "{version}");
        }
        Reply::Done {} => {}
        Reply::LogFilter { directives } => {
            let _ = writeln!(text, "{directives}");
        }
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
        Reply::ConfigFiles {
            files,
            styles,
            layouts,
            style_options,
            problems,
        } => config_files_lines(
            &mut text,
            files,
            ([styles, layouts], style_options),
            problems,
        ),
        Reply::ConfigSchema { schema } => {
            let _ = writeln!(
                text,
                "{}",
                serde_json::to_string_pretty(schema).unwrap_or_default()
            );
        }
        Reply::ConfigRead {
            value,
            changed,
            layout,
            from_layout,
            ..
        } => {
            if let Some(name) = layout {
                let _ = writeln!(text, "layout {name}; + marks what it supplies");
            }
            let mut path = Vec::new();
            setting_lines(&mut text, value, &mut path, (changed, from_layout));
        }
        Reply::Picked { choice } => {
            if let Some(choice) = choice {
                let _ = writeln!(text, "{choice}");
            }
        }
        Reply::Error { message } => {
            let _ = writeln!(text, "error: {message}");
        }
    }
    text
}

/// Appends the lines of a `config files` reply to `text`: each file and its
/// state, the `lists` of styles and layouts there are, and the
/// `problems` left out.
fn config_files_lines(
    text: &mut String,
    files: &[vaht_ipc::ConfigFileInfo],
    (lists, style_options): ([&[String]; 2], &[vaht_ipc::StyleOptionInfo]),
    problems: &[String],
) {
    for info in files {
        let state = match info.state {
            vaht_ipc::ConfigState::Clean => "clean",
            vaht_ipc::ConfigState::Changed => "changed",
        };
        let _ = writeln!(text, "{}\t{state}", info.file.file_name());
    }
    for (index, (kind, names)) in ["styles", "layouts"].iter().zip(lists).enumerate() {
        let gap = if index == 0 { "\n" } else { "" };
        let _ = writeln!(text, "{gap}{kind}: {}", names.join(", "));
    }
    for option in style_options {
        let choices: Vec<String> = option
            .choices
            .iter()
            .map(|choice| {
                if *choice == option.chosen {
                    format!("*{choice}")
                } else {
                    choice.clone()
                }
            })
            .collect();
        let _ = writeln!(text, "style option {}: {}", option.name, choices.join(", "));
    }
    if !problems.is_empty() {
        let _ = writeln!(
            text,
            "\n{} problem(s) in the configuration, left out:",
            problems.len()
        );
        for problem in problems {
            let _ = writeln!(text, "  {problem}");
        }
    }
}

/// Appends a `key = value` line to `text` for each setting in `value`,
/// reached through `path`, `*` marking those changed in place and `+` those
/// the layout supplies (`marks`, in that order); a setting with no value is
/// written `unset`.
fn setting_lines(
    text: &mut String,
    value: &serde_json::Value,
    path: &mut Vec<String>,
    marks: (&[Vec<String>], &[Vec<String>]),
) {
    match value {
        serde_json::Value::Object(table) if !table.is_empty() => {
            for (key, inner) in table {
                path.push(key.clone());
                setting_lines(text, inner, path, marks);
                path.pop();
            }
        }
        serde_json::Value::Null => write_setting(text, path, "unset", marks),
        scalar @ (serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_)
        | serde_json::Value::Array(_)
        | serde_json::Value::Object(_)) => write_setting(text, path, &scalar.to_string(), marks),
    }
}

/// Appends the line of the setting at `path`, whose value is `shown`.
fn write_setting(
    text: &mut String,
    path: &[String],
    shown: &str,
    (changed, from_layout): (&[Vec<String>], &[Vec<String>]),
) {
    let mark = if changed.iter().any(|key| key == path) {
        '*'
    } else if from_layout.iter().any(|key| key == path) {
        '+'
    } else {
        ' '
    };
    let _ = writeln!(text, "{mark} {} = {shown}", path.join("."));
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
                parent: None,
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

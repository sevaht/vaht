//! Commands: what a binding, and later a menu item or an IPC request, does
//! (02 §4, 01 §8).
//!
//! A command is written as an action string (`"exec kitty"`), an action
//! table (`{ exec = "kitty" }`), an array meaning a sequence, or a table for
//! a structured form (`{ repeat = "..." }`). Both action forms go through
//! one parser, [`Action::parse`]: a table is turned into the same name and
//! arguments a string has, so the two cannot disagree.

use std::fmt;
use std::num::NonZeroU32;

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};

use crate::window::{HorizontalEdge, ResizeEdges, VerticalEdge};

/// Actions of 02 §4 that vaht does not have yet. Naming one is an error that
/// says so, rather than "unknown action", and nothing is silently ignored.
const PLANNED_ACTIONS: [&str; 52] = [
    "activate-tray-item",
    "activate-window-list-entry",
    "add-workspace",
    "apply-profile",
    "arrange",
    "command-dialog",
    "custom-menu",
    "cycle-primary-output",
    "cycle-profile",
    "focus-direction",
    "fullscreen",
    "go-to-marked-window",
    "hide-menus",
    "key-mode",
    "mark-window",
    "maximize",
    "minimize",
    "move",
    "move-to",
    "next-window",
    "previous-window",
    "reload-theme",
    "remove-last-workspace",
    "rename-workspace",
    "resize",
    "resize-to",
    "restart",
    "root-menu",
    "run-dialog",
    "save-settings",
    "send-to-output",
    "send-to-workspace",
    "set-alpha",
    "set-layer",
    "set-setting",
    "set-theme",
    "set-title",
    "shade",
    "show-desktop",
    "snap-step",
    "stick",
    "take-to-workspace",
    "taskbar-to-output",
    "tile",
    "toggle-frame",
    "toggle-taskbar",
    "tray-item-menu",
    "window-list-menu",
    "window-menu",
    "workspace",
    "workspace-menu",
    "workspace-name",
];

/// Structured command forms of 02 §4 that vaht does not have yet.
const PLANNED_FORMS: [&str; 5] = ["toggle", "if", "for_each", "delay_ms", "action"];

/// What to do: an action, or several, or one repeated while a key is held.
///
/// Commands are immutable values; anything that changes while vaht runs (a
/// repeat in progress) is kept by the compositor, so a reload starts it
/// afresh (01 §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    /// One action.
    Run(Action),
    /// Several commands, in order (Fluxbox's `MacroCmd`).
    Sequence(Vec<Command>),
    /// A command run on a key press and again while the key is held
    /// (Fluxbox's `Repeat`); anywhere else it runs once.
    Repeat {
        /// What is repeated.
        command: Box<Command>,
        /// Repeats per second; the keyboard's own rate when absent.
        rate: Option<NonZeroU32>,
    },
}

impl Command {
    /// Returns whether this command repeats while its key is held.
    pub(crate) const fn repeats(&self) -> bool {
        matches!(self, Self::Repeat { .. })
    }
}

impl<'de> serde::Deserialize<'de> for Command {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(CommandVisitor)
    }
}

/// Reads a command in any of its forms.
struct CommandVisitor;

impl<'de> Visitor<'de> for CommandVisitor {
    type Value = Command;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a command: an action string, an array of commands, or a table")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Command, E> {
        Action::parse(text).map(Command::Run).map_err(E::custom)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Command, A::Error> {
        let mut commands = Vec::new();
        while let Some(command) = sequence.next_element()? {
            commands.push(command);
        }
        if commands.is_empty() {
            return Err(de::Error::custom("an empty array of commands does nothing"));
        }
        Ok(Command::Sequence(commands))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Command, A::Error> {
        let mut entries: Vec<(String, toml::Value)> = Vec::new();
        while let Some(entry) = map.next_entry()? {
            entries.push(entry);
        }
        command_from_table(entries).map_err(de::Error::custom)
    }
}

/// Builds a command from a table's `entries`: a structured form, or an
/// action written as `{ name = arguments }`.
fn command_from_table(entries: Vec<(String, toml::Value)>) -> Result<Command, String> {
    if let Some((_, repeated)) = entries.iter().find(|(key, _)| key == "repeat") {
        let mut rate = None;
        for (key, value) in &entries {
            match key.as_str() {
                "repeat" => {}
                "rate" => rate = Some(repeat_rate(value)?),
                other => return Err(format!("a repeat has no `{other}`; it takes `rate`")),
            }
        }
        let command: Command = repeated
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| error.message().to_owned())?;
        if command.repeats() {
            return Err("a repeat inside a repeat".to_owned());
        }
        return Ok(Command::Repeat {
            command: Box::new(command),
            rate,
        });
    }
    let mut entries = entries.into_iter();
    let (Some((name, value)), None) = (entries.next(), entries.next()) else {
        return Err(
            "an action table holds exactly one action, such as { exec = \"kitty\" }".to_owned(),
        );
    };
    if PLANNED_FORMS.contains(&name.as_str()) {
        return Err(format!("`{name}` is not available in vaht yet"));
    }
    let arguments = arguments_from_value(&name, &value)?;
    Action::from_parts(&name, &arguments).map(Command::Run)
}

/// Reads a repeat's `rate`: a whole number of repeats per second.
fn repeat_rate(value: &toml::Value) -> Result<NonZeroU32, String> {
    value
        .as_integer()
        .and_then(|rate| u32::try_from(rate).ok())
        .and_then(NonZeroU32::new)
        .ok_or_else(|| "a repeat's `rate` is a whole number of repeats per second".to_owned())
}

/// Returns the arguments an action table's `value` gives action `name`, as
/// the string form would write them.
fn arguments_from_value(name: &str, value: &toml::Value) -> Result<String, String> {
    match value {
        toml::Value::String(text) => Ok(text.clone()),
        toml::Value::Integer(number) => Ok(number.to_string()),
        toml::Value::Table(table) if table.is_empty() => Ok(String::new()),
        toml::Value::Table(table) if name == "set-environment" => {
            let field = |key: &str| {
                table.get(key).and_then(toml::Value::as_str).ok_or_else(|| {
                    "set-environment takes { name = \"NAME\", value = \"value\" }".to_owned()
                })
            };
            if table.len() != 2 {
                return Err("set-environment takes only `name` and `value`".to_owned());
            }
            Ok(format!("{} {}", field("name")?, field("value")?))
        }
        toml::Value::Table(_)
        | toml::Value::Float(_)
        | toml::Value::Boolean(_)
        | toml::Value::Datetime(_)
        | toml::Value::Array(_) => Err(format!(
            "the arguments of `{name}` are a string, a number, or {{}} for none"
        )),
    }
}

/// One thing vaht can do. The table form's key and the string form's first
/// word are the action's name (02 §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Runs a command line with `sh -c` (`exec <command line>`).
    Exec(String),
    /// Asks the window to close.
    Close,
    /// Ends the window's program.
    Kill,
    /// Moves the window above all others.
    Raise,
    /// Moves the window below all others.
    Lower,
    /// Gives the window keyboard focus.
    Focus,
    /// Moves the window with the pointer while the button that started it
    /// is held; only a mouse binding can start it.
    StartMoving,
    /// Resizes the window with the pointer while the button that started
    /// it is held; only a mouse binding can start it.
    StartResizing(ResizeFrom),
    /// Reads the configuration again.
    Reload,
    /// Stops vaht.
    Exit,
    /// Sets an environment variable for programs vaht starts from now on.
    SetEnvironment {
        /// The variable.
        name: String,
        /// Its value.
        value: String,
    },
}

impl Action {
    /// Parses the string form: the action's name, then its arguments.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (name, arguments) = text
            .split_once(char::is_whitespace)
            .map_or((text, ""), |(name, arguments)| (name, arguments.trim()));
        Self::from_parts(name, arguments)
    }

    /// Builds the action `name` with `arguments`, as written after it.
    fn from_parts(name: &str, arguments: &str) -> Result<Self, String> {
        let without_arguments = |action: Self| {
            if arguments.is_empty() {
                Ok(action)
            } else {
                Err(format!("`{name}` takes no arguments"))
            }
        };
        match name {
            "exec" if arguments.is_empty() => Err("`exec` needs a command line".to_owned()),
            "exec" => Ok(Self::Exec(arguments.to_owned())),
            "close" => without_arguments(Self::Close),
            "kill" => without_arguments(Self::Kill),
            "raise" => without_arguments(Self::Raise),
            "lower" => without_arguments(Self::Lower),
            "focus" => without_arguments(Self::Focus),
            "start-moving" => without_arguments(Self::StartMoving),
            "start-resizing" => ResizeFrom::parse(arguments).map(Self::StartResizing),
            "reload" => without_arguments(Self::Reload),
            "exit" => without_arguments(Self::Exit),
            "set-environment" => {
                let (variable, value) = arguments
                    .split_once(char::is_whitespace)
                    .map_or((arguments, ""), |(variable, value)| {
                        (variable, value.trim())
                    });
                if variable.is_empty() || variable.contains('=') {
                    return Err(
                        "`set-environment` needs a variable name, then its value".to_owned()
                    );
                }
                Ok(Self::SetEnvironment {
                    name: variable.to_owned(),
                    value: value.to_owned(),
                })
            }
            _ if PLANNED_ACTIONS.contains(&name) => {
                Err(format!("`{name}` is not available in vaht yet"))
            }
            _ => Err(format!("unknown action `{name}`")),
        }
    }
}

/// The string form, which is what vaht writes (02 §4).
impl fmt::Display for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exec(command_line) => write!(formatter, "exec {command_line}"),
            Self::Close => formatter.write_str("close"),
            Self::Kill => formatter.write_str("kill"),
            Self::Raise => formatter.write_str("raise"),
            Self::Lower => formatter.write_str("lower"),
            Self::Focus => formatter.write_str("focus"),
            Self::StartMoving => formatter.write_str("start-moving"),
            Self::StartResizing(from) => write!(formatter, "start-resizing {from}"),
            Self::Reload => formatter.write_str("reload"),
            Self::Exit => formatter.write_str("exit"),
            Self::SetEnvironment { name, value } => {
                write!(formatter, "set-environment {name} {value}")
            }
        }
    }
}

/// Which edges a resize started by `start-resizing` drags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResizeFrom {
    /// The corner nearest the pointer (the default).
    NearestCorner,
    /// The given edges.
    Edges(ResizeEdges),
}

impl ResizeFrom {
    /// The argument names and the edges they mean.
    const EDGES: [(&str, HorizontalEdge, VerticalEdge); 8] = [
        ("top-left", HorizontalEdge::Left, VerticalEdge::Top),
        ("top", HorizontalEdge::None, VerticalEdge::Top),
        ("top-right", HorizontalEdge::Right, VerticalEdge::Top),
        ("left", HorizontalEdge::Left, VerticalEdge::None),
        ("right", HorizontalEdge::Right, VerticalEdge::None),
        ("bottom-left", HorizontalEdge::Left, VerticalEdge::Bottom),
        ("bottom", HorizontalEdge::None, VerticalEdge::Bottom),
        ("bottom-right", HorizontalEdge::Right, VerticalEdge::Bottom),
    ];

    /// Parses `start-resizing`'s argument; none means the nearest corner.
    fn parse(argument: &str) -> Result<Self, String> {
        if argument.is_empty() || argument == "nearest-corner" {
            return Ok(Self::NearestCorner);
        }
        Self::EDGES
            .iter()
            .find(|(name, ..)| *name == argument)
            .map(|&(_, horizontal, vertical)| {
                Self::Edges(ResizeEdges {
                    horizontal,
                    vertical,
                })
            })
            .ok_or_else(|| {
                format!(
                    "`start-resizing` takes nearest-corner, top-left, top, top-right, left, \
                     right, bottom-left, bottom or bottom-right, not `{argument}`"
                )
            })
    }
}

impl fmt::Display for ResizeFrom {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::NearestCorner => "nearest-corner",
            Self::Edges(edges) => Self::EDGES
                .iter()
                .find(|&&(_, horizontal, vertical)| {
                    edges.horizontal == horizontal && edges.vertical == vertical
                })
                .map_or("nearest-corner", |(name, ..)| *name),
        };
        formatter.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `text` as the value of a TOML key.
    fn command(text: &str) -> Result<Command, String> {
        #[derive(serde::Deserialize)]
        struct Holder {
            /// The command.
            command: Command,
        }
        toml::from_str::<Holder>(&format!("command = {text}"))
            .map(|holder| holder.command)
            .map_err(|error| error.message().to_owned())
    }

    /// One of each action, with arguments where it takes them.
    fn every_action() -> Vec<Action> {
        let mut actions = vec![
            Action::Exec("kitty --single-instance".to_owned()),
            Action::Close,
            Action::Kill,
            Action::Raise,
            Action::Lower,
            Action::Focus,
            Action::StartMoving,
            Action::StartResizing(ResizeFrom::NearestCorner),
            Action::Reload,
            Action::Exit,
            Action::SetEnvironment {
                name: "GDK_SCALE".to_owned(),
                value: "2".to_owned(),
            },
        ];
        actions.extend(ResizeFrom::EDGES.iter().map(|&(_, horizontal, vertical)| {
            Action::StartResizing(ResizeFrom::Edges(ResizeEdges {
                horizontal,
                vertical,
            }))
        }));
        actions
    }

    #[test]
    fn every_action_reads_back_what_it_writes() {
        for action in every_action() {
            assert_eq!(Action::parse(&action.to_string()), Ok(action.clone()));
        }
    }

    #[test]
    fn the_string_and_table_forms_agree() {
        let pairs = [
            ("\"close\"", "{ close = {} }"),
            ("\"exec kitty\"", "{ exec = \"kitty\" }"),
            (
                "\"start-resizing bottom-left\"",
                "{ start-resizing = \"bottom-left\" }",
            ),
            (
                "\"set-environment GDK_SCALE 2\"",
                "{ set-environment = { name = \"GDK_SCALE\", value = \"2\" } }",
            ),
        ];
        for (string, table) in pairs {
            assert_eq!(command(string), command(table), "{string} and {table}");
            assert!(command(string).is_ok(), "{string}");
        }
    }

    #[test]
    fn an_array_is_a_sequence_and_repeat_wraps_a_command() {
        assert_eq!(
            command("[\"raise\", \"focus\"]"),
            Ok(Command::Sequence(vec![
                Command::Run(Action::Raise),
                Command::Run(Action::Focus),
            ]))
        );
        assert_eq!(
            command("{ repeat = \"exec amixer sset Master 2%+\", rate = 20 }"),
            Ok(Command::Repeat {
                command: Box::new(Command::Run(Action::Exec(
                    "amixer sset Master 2%+".to_owned()
                ))),
                rate: NonZeroU32::new(20),
            })
        );
    }

    #[test]
    fn mistakes_are_explained() {
        let cases = [
            ("\"explode\"", "unknown action `explode`"),
            ("\"maximize\"", "not available in vaht yet"),
            ("{ toggle = [\"shade\"] }", "not available in vaht yet"),
            ("\"close now\"", "takes no arguments"),
            ("\"exec\"", "needs a command line"),
            ("[]", "does nothing"),
            ("{ close = {}, raise = {} }", "exactly one action"),
            ("{ repeat = \"raise\", rate = 0 }", "whole number"),
            ("\"start-resizing middle\"", "not `middle`"),
        ];
        for (text, expected) in cases {
            let error = command(text).expect_err(text);
            assert!(error.contains(expected), "{text}: {error}");
        }
    }
}

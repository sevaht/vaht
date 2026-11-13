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
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::matcher::WindowMatcher;
use crate::window::{Axes, Direction, HorizontalEdge, ResizeEdges, TileCell, VerticalEdge};
use crate::workspaces::WorkspaceTarget;

/// Actions of 02 §4 that vaht does not have yet. Naming one is an error that
/// says so, rather than "unknown action", and nothing is silently ignored.
const PLANNED_ACTIONS: &[&str] = &[
    "activate-tray-item",
    "activate-window-list-entry",
    "apply-profile",
    "arrange",
    "command-dialog",
    "custom-menu",
    "cycle-primary-output",
    "cycle-profile",
    "focus-direction",
    "go-to-marked-window",
    "hide-menus",
    "key-mode",
    "mark-window",
    "reload-theme",
    "rename-workspace",
    "restart",
    "root-menu",
    "run-dialog",
    "save-settings",
    "send-to-output",
    "set-alpha",
    "set-layer",
    "set-setting",
    "set-theme",
    "set-title",
    "taskbar-to-output",
    "toggle-frame",
    "toggle-taskbar",
    "tray-item-menu",
    "window-list-menu",
    "window-menu",
    "workspace-menu",
];

/// Structured command forms of 02 §4 that vaht does not have yet.
const PLANNED_FORMS: [&str; 4] = ["toggle", "if", "for_each", "delay_ms"];

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

    /// Builds a command from a table's `entries`: a structured form, or an
    /// action written as `{ name = arguments }`.
    fn from_table(entries: Vec<(String, toml::Value)>) -> Result<Self, String> {
        if let Some((_, repeated)) = entries.iter().find(|(key, _)| key == "repeat") {
            let mut rate = None;
            for (key, value) in &entries {
                match key.as_str() {
                    "repeat" => {}
                    "rate" => rate = Some(Self::repeat_rate(value)?),
                    other => return Err(format!("a repeat has no `{other}`; it takes `rate`")),
                }
            }
            let command: Self = repeated
                .clone()
                .try_into()
                .map_err(|error: toml::de::Error| error.message().to_owned())?;
            if command.repeats() {
                return Err("a repeat inside a repeat".to_owned());
            }
            return Ok(Self::Repeat {
                command: Box::new(command),
                rate,
            });
        }
        if entries.iter().any(|(key, _)| key == "action") {
            return Self::from_action_table(entries);
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
        let arguments = Action::arguments_from_value(&name, &value)?;
        Action::from_parts(&name, &arguments).map(Self::Run)
    }

    /// Builds the command of `{ action = "...", windows = matcher }`: an
    /// action, with the windows it is about.
    fn from_action_table(entries: Vec<(String, toml::Value)>) -> Result<Self, String> {
        let mut action = None;
        let mut windows = None;
        for (key, value) in entries {
            match key.as_str() {
                "action" => {
                    let text = value
                        .as_str()
                        .ok_or("`action` is an action string, such as \"next-window\"")?;
                    action = Some(Action::parse(text)?);
                }
                "windows" => {
                    windows = Some(
                        value
                            .try_into::<WindowMatcher>()
                            .map_err(|error| format!("windows: {}", error.message()))?,
                    );
                }
                other => {
                    return Err(format!(
                        "a table with `action` takes only `windows` beside it, not `{other}`"
                    ));
                }
            }
        }
        let action = action.ok_or("`action` is missing")?;
        action.with_windows(windows).map(Self::Run)
    }

    /// Reads a repeat's `rate`: a whole number of repeats per second.
    fn repeat_rate(value: &toml::Value) -> Result<NonZeroU32, String> {
        value
            .as_integer()
            .and_then(|rate| u32::try_from(rate).ok())
            .and_then(NonZeroU32::new)
            .ok_or_else(|| "a repeat's `rate` is a whole number of repeats per second".to_owned())
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
        Command::from_table(entries).map_err(de::Error::custom)
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
    /// Toggles maximizing the window along the axes (`maximize`, or
    /// `maximize horizontal|vertical`).
    Maximize(Axes),
    /// Minimizes the window.
    Minimize,
    /// Toggles the window covering the whole output.
    Fullscreen,
    /// Toggles rolling the window up into its titlebar.
    Shade,
    /// Toggles showing the window on every workspace.
    Stick,
    /// Places the window on a cell of the usable area, given in thousandths
    /// of its width and height (`tile <x> <y> <width> <height>`; the left
    /// half is `tile 0 0 500 1000`).
    Tile(TileCell),
    /// Takes one step of the snap ladder (`snap-step left|right|up|down`).
    SnapStep(Direction),
    /// Moves the window by a distance (`move <dx> <dy>`).
    Move(Point<i32, Logical>),
    /// Moves the window's frame to a position (`move-to <x> <y>`).
    MoveTo(Point<i32, Logical>),
    /// Grows the window's content by a width and height, which may be
    /// negative (`resize <dw> <dh>`).
    Resize(Point<i32, Logical>),
    /// Sizes the window's content (`resize-to <width> <height>`).
    ResizeTo(Size<i32, Logical>),
    /// Shows a workspace (`workspace <n|next|previous|last>`).
    Workspace(WorkspaceTarget),
    /// Puts the window on a workspace (`send-to-workspace <target>`).
    SendToWorkspace(WorkspaceTarget),
    /// Puts the window on a workspace and shows that workspace
    /// (`take-to-workspace <target>`).
    TakeToWorkspace(WorkspaceTarget),
    /// Adds a workspace at the end.
    AddWorkspace,
    /// Removes the last workspace; its windows move to the one before.
    RemoveLastWorkspace,
    /// Focuses and raises the next window, in the order windows last had
    /// focus, among those matching.
    NextWindow(WindowMatcher),
    /// Focuses and raises the previous window among those matching.
    PreviousWindow(WindowMatcher),
    /// Minimizes every window on the workspace, or restores them if they
    /// all are.
    ShowDesktop,
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
            "maximize" if arguments.is_empty() => Ok(Self::Maximize(Axes::Both)),
            "maximize" => match arguments.parse() {
                Ok(axes @ (Axes::Horizontal | Axes::Vertical)) => Ok(Self::Maximize(axes)),
                Ok(Axes::Both) | Err(_) => Err(format!(
                    "`maximize` takes nothing, horizontal or vertical, not `{arguments}`"
                )),
            },
            "minimize" => without_arguments(Self::Minimize),
            "fullscreen" => without_arguments(Self::Fullscreen),
            "shade" => without_arguments(Self::Shade),
            "stick" => without_arguments(Self::Stick),
            "snap-step" => arguments.parse().map(Self::SnapStep).map_err(|_unknown| {
                format!("`snap-step` takes left, right, up or down, not `{arguments}`")
            }),
            "tile" => Self::cell(arguments).map(Self::Tile),
            "move" => Self::pair(name, arguments).map(|(x, y)| Self::Move((x, y).into())),
            "move-to" => Self::pair(name, arguments).map(|(x, y)| Self::MoveTo((x, y).into())),
            "resize" => Self::pair(name, arguments).map(|(x, y)| Self::Resize((x, y).into())),
            "resize-to" => match Self::pair(name, arguments)? {
                (width, height) if width > 0 && height > 0 => {
                    Ok(Self::ResizeTo((width, height).into()))
                }
                _ => Err("`resize-to` takes a width and height above 0".to_owned()),
            },
            "workspace" => arguments.parse().map(Self::Workspace),
            "send-to-workspace" => arguments.parse().map(Self::SendToWorkspace),
            "take-to-workspace" => arguments.parse().map(Self::TakeToWorkspace),
            "add-workspace" => without_arguments(Self::AddWorkspace),
            "remove-last-workspace" => without_arguments(Self::RemoveLastWorkspace),
            "next-window" => without_arguments(Self::NextWindow(WindowMatcher::default())),
            "previous-window" => without_arguments(Self::PreviousWindow(WindowMatcher::default())),
            "show-desktop" => without_arguments(Self::ShowDesktop),
            _ if PLANNED_ACTIONS.contains(&name) => {
                Err(format!("`{name}` is not available in vaht yet"))
            }
            _ => Err(format!("unknown action `{name}`")),
        }
    }

    /// Returns the action about the windows `windows` matches, for the
    /// actions that are about a set of windows.
    fn with_windows(self, windows: Option<WindowMatcher>) -> Result<Self, String> {
        match (self, windows) {
            (action, None) => Ok(action),
            (Self::NextWindow(_), Some(windows)) => Ok(Self::NextWindow(windows)),
            (Self::PreviousWindow(_), Some(windows)) => Ok(Self::PreviousWindow(windows)),
            (action, Some(_)) => Err(format!("`{}` takes no `windows`", action.name())),
        }
    }

    /// Parses two whole numbers, as `move`, `move-to`, `resize` and
    /// `resize-to` take.
    fn pair(name: &str, arguments: &str) -> Result<(i32, i32), String> {
        let numbers: Vec<i32> = arguments
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_not_a_number| format!("`{name}` takes two whole numbers"))?;
        match numbers.as_slice() {
            [first, second] => Ok((*first, *second)),
            _ => Err(format!("`{name}` takes two whole numbers")),
        }
    }

    /// Parses `tile`'s cell: four thousandths of the usable area.
    fn cell(arguments: &str) -> Result<TileCell, String> {
        let numbers: Option<Vec<u16>> = arguments
            .split_whitespace()
            .map(|number| number.parse().ok())
            .collect();
        match numbers.as_deref() {
            Some(&[x, y, width, height]) => TileCell::new(x, y, width, height),
            Some(_) | None => None,
        }
        .ok_or_else(|| {
            "`tile` takes x, y, width and height in thousandths of the usable area, \
             within 0 to 1000, such as `tile 0 0 500 1000`"
                .to_owned()
        })
    }

    /// Returns the action's name, the first word of its string form.
    fn name(&self) -> String {
        let written = self.to_string();
        written
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned()
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
            Self::Maximize(Axes::Both) => formatter.write_str("maximize"),
            Self::Maximize(axes) => write!(formatter, "maximize {axes}"),
            Self::Minimize => formatter.write_str("minimize"),
            Self::Fullscreen => formatter.write_str("fullscreen"),
            Self::Shade => formatter.write_str("shade"),
            Self::Stick => formatter.write_str("stick"),
            Self::Tile(cell) => write!(formatter, "tile {cell}"),
            Self::SnapStep(direction) => write!(formatter, "snap-step {direction}"),
            Self::Move(distance) => write!(formatter, "move {} {}", distance.x, distance.y),
            Self::MoveTo(position) => write!(formatter, "move-to {} {}", position.x, position.y),
            Self::Resize(growth) => write!(formatter, "resize {} {}", growth.x, growth.y),
            Self::ResizeTo(size) => write!(formatter, "resize-to {} {}", size.w, size.h),
            Self::Workspace(target) => write!(formatter, "workspace {target}"),
            Self::SendToWorkspace(target) => write!(formatter, "send-to-workspace {target}"),
            Self::TakeToWorkspace(target) => write!(formatter, "take-to-workspace {target}"),
            Self::AddWorkspace => formatter.write_str("add-workspace"),
            Self::RemoveLastWorkspace => formatter.write_str("remove-last-workspace"),
            // The string form has no matcher; vaht writes a table for one.
            Self::NextWindow(_) => formatter.write_str("next-window"),
            Self::PreviousWindow(_) => formatter.write_str("previous-window"),
            Self::ShowDesktop => formatter.write_str("show-desktop"),
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

    /// Returns the edges a resize of a window whose frame covers `outer`
    /// drags, with the pointer at `pointer`.
    pub(crate) fn edges(
        self,
        outer: Rectangle<i32, Logical>,
        pointer: Point<f64, Logical>,
    ) -> ResizeEdges {
        match self {
            Self::NearestCorner => ResizeEdges::nearest_corner(outer, pointer),
            Self::Edges(edges) => edges,
        }
    }

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
    use rstest::rstest;

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
            Action::Maximize(Axes::Both),
            Action::Maximize(Axes::Vertical),
            Action::Minimize,
            Action::Fullscreen,
            Action::Shade,
            Action::Stick,
            Action::SnapStep(Direction::Left),
            Action::Tile(TileCell::new(500, 0, 500, 500).expect("a valid cell")),
            Action::Move((-10, 20).into()),
            Action::MoveTo((0, 0).into()),
            Action::Resize((-30, 5).into()),
            Action::ResizeTo((800, 600).into()),
            Action::Workspace(WorkspaceTarget::Last),
            Action::SendToWorkspace(WorkspaceTarget::Numbered(
                std::num::NonZeroUsize::new(3).expect("not zero"),
            )),
            Action::TakeToWorkspace(WorkspaceTarget::Next),
            Action::AddWorkspace,
            Action::RemoveLastWorkspace,
            Action::NextWindow(WindowMatcher::default()),
            Action::PreviousWindow(WindowMatcher::default()),
            Action::ShowDesktop,
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

    #[rstest]
    #[case::without_arguments("\"close\"", "{ close = {} }")]
    #[case::with_a_string("\"exec kitty\"", "{ exec = \"kitty\" }")]
    #[case::with_a_word(
        "\"start-resizing bottom-left\"",
        "{ start-resizing = \"bottom-left\" }"
    )]
    #[case::with_a_table(
        "\"set-environment GDK_SCALE 2\"",
        "{ set-environment = { name = \"GDK_SCALE\", value = \"2\" } }"
    )]
    fn the_string_and_table_forms_agree(#[case] string: &str, #[case] table: &str) {
        assert!(command(string).is_ok(), "{string}");
        assert_eq!(command(string), command(table));
    }

    #[test]
    fn an_action_table_carries_its_windows() {
        let parsed = command("{ action = \"next-window\", windows = { workspace = \"current\" } }")
            .expect("valid");
        let Command::Run(Action::NextWindow(windows)) = parsed else {
            panic!("next-window with a matcher");
        };
        assert_ne!(windows, WindowMatcher::default());
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

    #[rstest]
    #[case::unknown_action("\"explode\"", "unknown action `explode`")]
    #[case::planned_action("\"root-menu\"", "not available in vaht yet")]
    #[case::maximize_both_by_name("\"maximize both\"", "nothing, horizontal or vertical")]
    #[case::one_number("\"move 10\"", "two whole numbers")]
    #[case::cell_outside("\"tile 600 0 500 1000\"", "within 0 to 1000")]
    #[case::empty_size("\"resize-to 0 300\"", "above 0")]
    #[case::workspace_zero("\"workspace 0\"", "a workspace is a number")]
    #[case::windows_on_close("{ action = \"close\", windows = {} }", "takes no `windows`")]
    #[case::bad_matcher(
        "{ action = \"next-window\", windows = { colour = 1 } }",
        "unknown field"
    )]
    #[case::planned_form("{ toggle = [\"shade\"] }", "not available in vaht yet")]
    #[case::unwanted_arguments("\"close now\"", "takes no arguments")]
    #[case::missing_command_line("\"exec\"", "needs a command line")]
    #[case::empty_sequence("[]", "does nothing")]
    #[case::two_actions_in_a_table("{ close = {}, raise = {} }", "exactly one action")]
    #[case::zero_rate("{ repeat = \"raise\", rate = 0 }", "whole number")]
    #[case::unknown_corner("\"start-resizing middle\"", "not `middle`")]
    fn mistakes_are_explained(#[case] text: &str, #[case] expected: &str) {
        let error = command(text).expect_err(text);
        assert!(error.contains(expected), "{error}");
    }
}

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
use crate::window_manager::{
    AlphaChange, Axes, Direction, HorizontalEdge, OutputTarget, ResizeEdges, StackLayer, TileCell,
    VerticalEdge, WorkspaceTarget,
};

/// Actions of 02 §4 that vaht does not have yet. Naming one is an error that
/// says so, rather than "unknown action", and nothing is silently ignored.
const PLANNED_ACTIONS: &[&str] = &[
    "activate-tray-item",
    "activate-window-list-entry",
    "arrange",
    "key-mode",
    "reload-theme",
    "rename-workspace",
    "restart",
    "set-setting",
    "taskbar-to-output",
    "toggle-frame",
    "toggle-taskbar",
    "tray-item-menu",
];

/// Structured command forms of 02 §4 that vaht does not have yet.
const PLANNED_FORMS: [&str; 4] = ["toggle", "if", "for_each", "delay_ms"];

/// The windows `next-window` and `previous-window` go through, and in what
/// order: `{ action = "next-window", windows = <matcher>, order = "opening" }`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct WindowCycle {
    /// Which windows.
    pub(crate) windows: WindowMatcher,
    /// In what order.
    pub(crate) order: CycleOrder,
}

/// The order a window cycle goes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CycleOrder {
    /// The order windows last had focus in, most recent first; a cycle
    /// keeps its order until the keys held for it are let go.
    #[default]
    Focus,
    /// The order windows opened in, as the taskbar lists them (Fluxbox's
    /// `{static}`).
    Opening,
}

impl CycleOrder {
    /// Reads `focus` or `opening`.
    fn parse(text: &str) -> Result<Self, String> {
        match text {
            "focus" => Ok(Self::Focus),
            "opening" => Ok(Self::Opening),
            other => Err(format!("`order` is focus or opening, not `{other}`")),
        }
    }
}

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
    /// Returns the names of the menus `custom-menu` opens in this command,
    /// its parts included.
    pub(crate) fn custom_menus(&self) -> Vec<&str> {
        match self {
            Self::Run(Action::OpenMenu(MenuRequest::Custom(name))) => vec![name.as_str()],
            Self::Run(_) => Vec::new(),
            Self::Sequence(commands) => commands.iter().flat_map(Self::custom_menus).collect(),
            Self::Repeat { command, .. } => command.custom_menus(),
        }
    }

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
        let mut order = None;
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
                "order" => {
                    let text = value.as_str().ok_or("`order` is focus or opening")?;
                    order = Some(CycleOrder::parse(text)?);
                }
                other => {
                    return Err(format!(
                        "a table with `action` takes only `windows` and `order` beside it, \
                         not `{other}`"
                    ));
                }
            }
        }
        let action = action.ok_or("`action` is missing")?;
        action.with_cycle(windows, order).map(Self::Run)
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
    /// Window management, which the window manager carries out.
    WindowManager(WindowManagerAction),
    /// Runs a command line with `sh -c` (`exec <command line>`).
    Exec(String),
    /// Moves the window with the pointer while the button that started it
    /// is held; only a mouse binding can start it.
    StartMoving,
    /// Resizes the window with the pointer while the button that started
    /// it is held; only a mouse binding can start it.
    StartResizing(ResizeFrom),
    /// Reads the configuration again.
    Reload,
    /// Shows the run dialog: a command line to type and run.
    RunDialog,
    /// Shows the command dialog: a vaht command to type and run.
    CommandDialog,
    /// Shows the title dialog for the window: its title, to change.
    SetTitleDialog,
    /// Uses a theme in place of the settings' (`set-theme <name>`), until
    /// `save-settings` writes it to `settings.toml`.
    SetTheme(String),
    /// Writes the settings changed in place into the files.
    SaveSettings,
    /// Applies the display profile with this name (`outputs.toml`).
    ApplyProfile(String),
    /// Applies the next display profile whose outputs are connected.
    CycleProfile,
    /// Makes the next output primary.
    CyclePrimaryOutput,
    /// Makes the output on this connector primary.
    SetPrimaryOutput(String),
    /// Stops vaht.
    Exit,
    /// Sets an environment variable for programs vaht starts from now on.
    SetEnvironment {
        /// The variable.
        name: String,
        /// Its value.
        value: String,
    },
    /// Opens a menu at the pointer, closing any open.
    OpenMenu(MenuRequest),
    /// Closes every menu.
    HideMenus,
}

/// Which menu an action opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuRequest {
    /// `[menu.root]` (`root-menu`).
    Root,
    /// `[menu.window]`, about the window (`window-menu`).
    Window,
    /// `[menu.workspace]` (`workspace-menu`).
    Workspace,
    /// Every window (`window-list-menu`).
    WindowList,
    /// `[menu.<name>]` (`custom-menu <name>`).
    Custom(String),
}

/// An action on windows, workspaces or focus: everything the window
/// manager decides alone ([`crate::window_manager::WindowManager::run`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WindowManagerAction {
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
    /// Focuses and raises the next window, among those matching, in the
    /// cycle's order.
    NextWindow(WindowCycle),
    /// Focuses and raises the previous window among those matching.
    PreviousWindow(WindowCycle),
    /// Minimizes every window on the workspace, or restores them if they
    /// all are.
    ShowDesktop,
    /// Gives the window a title of vaht's (`set-title <title>`), or its
    /// program's again (`set-title`).
    SetTitle(Option<String>),
    /// Moves the window to a stacking layer (`set-layer <layer>`:
    /// above-dock, dock, top, normal, bottom or desktop).
    SetLayer(StackLayer),
    /// Changes the window's opacity (`set-alpha [<both> | <focused>
    /// <unfocused>]`, each 0 to 255 or a change like `+10`; nothing goes
    /// back to the settings').
    SetAlpha(AlphaChange),
    /// Focuses and raises the nearest window shown in a direction from the
    /// focused one (`focus-direction left|right|up|down`).
    FocusDirection(Direction),
    /// Marks the window with a key, any word (`mark-window <key>`); a key
    /// marks one window at a time.
    MarkWindow(String),
    /// Brings forward the window a key marks (`go-to-marked-window <key>`).
    GoToMarkedWindow(String),
    /// Moves the window to another output (`send-to-output
    /// next|previous|<head>`, heads numbered from 1 left to right).
    SendToOutput(OutputTarget),
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
        let without_arguments = |action: Self| no_arguments(name, arguments).map(|()| action);
        match name {
            "exec" if arguments.is_empty() => Err("`exec` needs a command line".to_owned()),
            "exec" => Ok(Self::Exec(arguments.to_owned())),
            "start-moving" => without_arguments(Self::StartMoving),
            "start-resizing" => ResizeFrom::parse(arguments).map(Self::StartResizing),
            "reload" => without_arguments(Self::Reload),
            "run-dialog" => without_arguments(Self::RunDialog),
            "command-dialog" => without_arguments(Self::CommandDialog),
            "set-title-dialog" => without_arguments(Self::SetTitleDialog),
            "set-theme" if arguments.is_empty() => {
                Err("`set-theme` needs a theme's name".to_owned())
            }
            "set-theme" => Ok(Self::SetTheme(arguments.to_owned())),
            "save-settings" => without_arguments(Self::SaveSettings),
            "apply-profile" if arguments.is_empty() => {
                Err("`apply-profile` needs a profile's name".to_owned())
            }
            "apply-profile" => Ok(Self::ApplyProfile(arguments.to_owned())),
            "cycle-profile" => without_arguments(Self::CycleProfile),
            "cycle-primary-output" => without_arguments(Self::CyclePrimaryOutput),
            "set-primary-output" if arguments.is_empty() => {
                Err("`set-primary-output` needs an output's connector, such as HDMI-A-1".to_owned())
            }
            "set-primary-output" => Ok(Self::SetPrimaryOutput(arguments.to_owned())),
            "root-menu" => without_arguments(Self::OpenMenu(MenuRequest::Root)),
            "window-menu" => without_arguments(Self::OpenMenu(MenuRequest::Window)),
            "workspace-menu" => without_arguments(Self::OpenMenu(MenuRequest::Workspace)),
            "window-list-menu" => without_arguments(Self::OpenMenu(MenuRequest::WindowList)),
            "custom-menu" if arguments.is_empty() => {
                Err("`custom-menu` needs the menu's name, as [menu.<name>] has it".to_owned())
            }
            "custom-menu" => Ok(Self::OpenMenu(MenuRequest::Custom(arguments.to_owned()))),
            "hide-menus" => without_arguments(Self::HideMenus),
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
            _ => WindowManagerAction::from_parts(name, arguments).map(Self::WindowManager),
        }
    }

    /// Returns the action going through the windows `windows` matches in
    /// `order`, for the actions that cycle through windows.
    fn with_cycle(
        self,
        windows: Option<WindowMatcher>,
        order: Option<CycleOrder>,
    ) -> Result<Self, String> {
        use WindowManagerAction::{NextWindow, PreviousWindow};
        if windows.is_none() && order.is_none() {
            return Ok(self);
        }
        let cycle = |given: WindowCycle| WindowCycle {
            windows: windows.unwrap_or(given.windows),
            order: order.unwrap_or(given.order),
        };
        if let Self::WindowManager(NextWindow(given)) = self {
            return Ok(Self::WindowManager(NextWindow(cycle(given))));
        }
        if let Self::WindowManager(PreviousWindow(given)) = self {
            return Ok(Self::WindowManager(PreviousWindow(cycle(given))));
        }
        Err(format!("`{}` takes no `windows` or `order`", self.name()))
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
            Self::WindowManager(action) => action.fmt(formatter),
            Self::Exec(command_line) => write!(formatter, "exec {command_line}"),
            Self::StartMoving => formatter.write_str("start-moving"),
            Self::StartResizing(from) => write!(formatter, "start-resizing {from}"),
            Self::Reload => formatter.write_str("reload"),
            Self::RunDialog => formatter.write_str("run-dialog"),
            Self::CommandDialog => formatter.write_str("command-dialog"),
            Self::SetTitleDialog => formatter.write_str("set-title-dialog"),
            Self::SetTheme(name) => write!(formatter, "set-theme {name}"),
            Self::SaveSettings => formatter.write_str("save-settings"),
            Self::ApplyProfile(name) => write!(formatter, "apply-profile {name}"),
            Self::CycleProfile => formatter.write_str("cycle-profile"),
            Self::CyclePrimaryOutput => formatter.write_str("cycle-primary-output"),
            Self::SetPrimaryOutput(connector) => {
                write!(formatter, "set-primary-output {connector}")
            }
            Self::Exit => formatter.write_str("exit"),
            Self::SetEnvironment { name, value } => {
                write!(formatter, "set-environment {name} {value}")
            }
            Self::OpenMenu(MenuRequest::Root) => formatter.write_str("root-menu"),
            Self::OpenMenu(MenuRequest::Window) => formatter.write_str("window-menu"),
            Self::OpenMenu(MenuRequest::Workspace) => formatter.write_str("workspace-menu"),
            Self::OpenMenu(MenuRequest::WindowList) => formatter.write_str("window-list-menu"),
            Self::OpenMenu(MenuRequest::Custom(name)) => write!(formatter, "custom-menu {name}"),
            Self::HideMenus => formatter.write_str("hide-menus"),
        }
    }
}

impl WindowManagerAction {
    /// Builds the window-management action `name` with `arguments`, as
    /// written after it.
    fn from_parts(name: &str, arguments: &str) -> Result<Self, String> {
        let without_arguments = |action: Self| no_arguments(name, arguments).map(|()| action);
        match name {
            "close" => without_arguments(Self::Close),
            "kill" => without_arguments(Self::Kill),
            "raise" => without_arguments(Self::Raise),
            "lower" => without_arguments(Self::Lower),
            "focus" => without_arguments(Self::Focus),
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
            "set-alpha" => AlphaChange::parse(arguments).map(Self::SetAlpha),
            "set-layer" => arguments.parse().map(Self::SetLayer),
            "set-title" => Ok(Self::SetTitle(
                (!arguments.is_empty()).then(|| arguments.to_owned()),
            )),
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
            "next-window" => without_arguments(Self::NextWindow(WindowCycle::default())),
            "previous-window" => without_arguments(Self::PreviousWindow(WindowCycle::default())),
            "show-desktop" => without_arguments(Self::ShowDesktop),
            "focus-direction" => arguments
                .parse()
                .map(Self::FocusDirection)
                .map_err(|_unknown| {
                    format!("`focus-direction` takes left, right, up or down, not `{arguments}`")
                }),
            "mark-window" | "go-to-marked-window" if arguments.is_empty() => {
                Err(format!("`{name}` needs a key, such as `mark-window a`"))
            }
            "mark-window" => Ok(Self::MarkWindow(arguments.to_owned())),
            "send-to-output" => match arguments {
                "next" => Ok(Self::SendToOutput(OutputTarget::Next)),
                "previous" => Ok(Self::SendToOutput(OutputTarget::Previous)),
                head => head
                    .parse::<std::num::NonZeroUsize>()
                    .map(|head| Self::SendToOutput(OutputTarget::Head(head)))
                    .map_err(|_not_a_head| {
                        format!(
                            "`send-to-output` takes next, previous or a head from 1, \
                             not `{arguments}`"
                        )
                    }),
            },
            "go-to-marked-window" => Ok(Self::GoToMarkedWindow(arguments.to_owned())),
            _ if PLANNED_ACTIONS.contains(&name) => {
                Err(format!("`{name}` is not available in vaht yet"))
            }
            _ => Err(format!("unknown action `{name}`")),
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
}

/// The string form, which is what vaht writes (02 §4).
impl fmt::Display for WindowManagerAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Close => formatter.write_str("close"),
            Self::Kill => formatter.write_str("kill"),
            Self::Raise => formatter.write_str("raise"),
            Self::Lower => formatter.write_str("lower"),
            Self::Focus => formatter.write_str("focus"),
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
            Self::SetAlpha(change) => write!(formatter, "{change}"),
            Self::SetLayer(layer) => write!(formatter, "set-layer {layer}"),
            Self::SetTitle(None) => formatter.write_str("set-title"),
            Self::SetTitle(Some(title)) => write!(formatter, "set-title {title}"),
            Self::FocusDirection(direction) => write!(formatter, "focus-direction {direction}"),
            Self::MarkWindow(key) => write!(formatter, "mark-window {key}"),
            Self::GoToMarkedWindow(key) => write!(formatter, "go-to-marked-window {key}"),
            Self::SendToOutput(OutputTarget::Next) => formatter.write_str("send-to-output next"),
            Self::SendToOutput(OutputTarget::Previous) => {
                formatter.write_str("send-to-output previous")
            }
            Self::SendToOutput(OutputTarget::Head(head)) => {
                write!(formatter, "send-to-output {head}")
            }
        }
    }
}

/// Returns an error unless `arguments`, written after action `name`, are
/// empty.
fn no_arguments(name: &str, arguments: &str) -> Result<(), String> {
    if arguments.is_empty() {
        Ok(())
    } else {
        Err(format!("`{name}` takes no arguments"))
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
        let managed = [
            WindowManagerAction::Close,
            WindowManagerAction::Kill,
            WindowManagerAction::Raise,
            WindowManagerAction::Lower,
            WindowManagerAction::Focus,
            WindowManagerAction::Maximize(Axes::Both),
            WindowManagerAction::Maximize(Axes::Vertical),
            WindowManagerAction::Minimize,
            WindowManagerAction::Fullscreen,
            WindowManagerAction::Shade,
            WindowManagerAction::Stick,
            WindowManagerAction::SnapStep(Direction::Left),
            WindowManagerAction::Tile(TileCell::new(500, 0, 500, 500).expect("a valid cell")),
            WindowManagerAction::Move((-10, 20).into()),
            WindowManagerAction::MoveTo((0, 0).into()),
            WindowManagerAction::Resize((-30, 5).into()),
            WindowManagerAction::ResizeTo((800, 600).into()),
            WindowManagerAction::Workspace(WorkspaceTarget::Last),
            WindowManagerAction::SendToWorkspace(WorkspaceTarget::Numbered(
                std::num::NonZeroUsize::new(3).expect("not zero"),
            )),
            WindowManagerAction::TakeToWorkspace(WorkspaceTarget::Next),
            WindowManagerAction::AddWorkspace,
            WindowManagerAction::RemoveLastWorkspace,
            WindowManagerAction::NextWindow(WindowCycle::default()),
            WindowManagerAction::PreviousWindow(WindowCycle::default()),
            WindowManagerAction::ShowDesktop,
        ];
        let mut actions = vec![
            Action::Exec("kitty --single-instance".to_owned()),
            Action::StartMoving,
            Action::StartResizing(ResizeFrom::NearestCorner),
            Action::Reload,
            Action::Exit,
            Action::SetEnvironment {
                name: "GDK_SCALE".to_owned(),
                value: "2".to_owned(),
            },
        ];
        actions.extend(managed.into_iter().map(Action::WindowManager));
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
        let Command::Run(Action::WindowManager(WindowManagerAction::NextWindow(cycle))) = parsed
        else {
            panic!("next-window with a matcher");
        };
        assert_ne!(cycle.windows, WindowMatcher::default());
        assert_eq!(cycle.order, CycleOrder::Focus);
        let parsed =
            command("{ action = \"previous-window\", order = \"opening\" }").expect("valid");
        let Command::Run(Action::WindowManager(WindowManagerAction::PreviousWindow(cycle))) =
            parsed
        else {
            panic!("previous-window in opening order");
        };
        assert_eq!(cycle.order, CycleOrder::Opening);
    }

    #[test]
    fn an_array_is_a_sequence_and_repeat_wraps_a_command() {
        assert_eq!(
            command("[\"raise\", \"focus\"]"),
            Ok(Command::Sequence(vec![
                Command::Run(Action::WindowManager(WindowManagerAction::Raise)),
                Command::Run(Action::WindowManager(WindowManagerAction::Focus)),
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
    #[case::planned_action("\"reload-theme\"", "not available in vaht yet")]
    #[case::maximize_both_by_name("\"maximize both\"", "nothing, horizontal or vertical")]
    #[case::one_number("\"move 10\"", "two whole numbers")]
    #[case::cell_outside("\"tile 600 0 500 1000\"", "within 0 to 1000")]
    #[case::empty_size("\"resize-to 0 300\"", "above 0")]
    #[case::workspace_zero("\"workspace 0\"", "a workspace is a number")]
    #[case::windows_on_close(
        "{ action = \"close\", windows = {} }",
        "takes no `windows` or `order`"
    )]
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

//! `menus.toml`: the root menu, the window menu and any others (02 §8).
//!
//! Menus are named tables (`[menu.<name>]`), each with an optional `title`
//! and its `items`. A submenu refers to another menu by name, which keeps
//! deep menus flat and readable. An item is one of a few forms
//! ([`MenuEntry`]): a command, a submenu, a toggle of a window state, a
//! generated list (the workspaces, the windows), a separator, or a bare
//! label. vaht's own menus are `root`, `window` and `workspace`; the
//! `custom-menu <name>` action opens any other.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use serde::de::{self, Deserializer};

use super::command::{Action, Command, WindowManagerAction};
use crate::window_manager::Axes;

/// Every menu, by name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Menus {
    /// The menus.
    by_name: HashMap<String, MenuDefinition>,
}

impl Menus {
    /// The menu `root-menu` opens.
    pub(crate) const ROOT: &str = "root";
    /// The menu `window-menu` opens.
    pub(crate) const WINDOW: &str = "window";
    /// The menu `workspace-menu` opens.
    pub(crate) const WORKSPACE: &str = "workspace";

    /// Returns every command the menus' items run.
    pub(crate) fn commands(&self) -> impl Iterator<Item = &Command> {
        self.by_name.values().flat_map(|menu| {
            menu.items.iter().filter_map(|entry| match entry {
                MenuEntry::Command { command, .. } => Some(command),
                MenuEntry::Submenu { .. }
                | MenuEntry::Pipe { .. }
                | MenuEntry::Toggle { .. }
                | MenuEntry::Generate { .. }
                | MenuEntry::Builtin { .. }
                | MenuEntry::Separator
                | MenuEntry::Inert { .. } => None,
            })
        })
    }

    /// Returns the icons the menus' items name, each once.
    pub(crate) fn icons(&self) -> Vec<&str> {
        let mut icons: Vec<&str> = self
            .by_name
            .values()
            .flat_map(|menu| &menu.items)
            .filter_map(MenuEntry::icon)
            .collect();
        icons.sort_unstable();
        icons.dedup();
        icons
    }

    /// Returns what each item that runs a program, and names no icon, is
    /// looked up by, each once.
    pub(crate) fn inferred_icons(&self) -> Vec<InferredIcon> {
        let mut inferred: Vec<InferredIcon> = self
            .by_name
            .values()
            .flat_map(|menu| &menu.items)
            .filter_map(MenuEntry::inferred_icon)
            .collect();
        inferred.sort_unstable();
        inferred.dedup();
        inferred
    }

    /// Returns what to look the icon `icon` up by: a style's icon name as
    /// it is, or a file's path, `~/` read as the home directory and a
    /// relative path taken from `config_directory`.
    pub(crate) fn icon_lookup(icon: &str, config_directory: &Path) -> Option<String> {
        if icon.contains('/') {
            super::settings::file_path(icon, config_directory)
                .map(|path| path.display().to_string())
        } else {
            Some(icon.to_owned())
        }
    }

    /// Returns the menu named `name`, if there is one.
    pub(crate) fn get(&self, name: &str) -> Option<&MenuDefinition> {
        self.by_name.get(name)
    }

    /// Returns the first submenu named that no menu is, with the menu
    /// naming it.
    fn missing_submenu(&self) -> Option<(&str, &str)> {
        self.by_name.iter().find_map(|(name, menu)| {
            menu.items.iter().find_map(|entry| match entry {
                MenuEntry::Submenu { menu: submenu, .. } if !self.by_name.contains_key(submenu) => {
                    Some((name.as_str(), submenu.as_str()))
                }
                MenuEntry::Submenu { .. }
                | MenuEntry::Command { .. }
                | MenuEntry::Pipe { .. }
                | MenuEntry::Toggle { .. }
                | MenuEntry::Generate { .. }
                | MenuEntry::Builtin { .. }
                | MenuEntry::Separator
                | MenuEntry::Inert { .. } => None,
            })
        })
    }
}

/// The file as written: `[menu.<name>]` tables.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MenusFile {
    /// The menus.
    #[serde(default)]
    menu: HashMap<String, MenuDefinition>,
}

impl Menus {
    /// Returns the menus of vaht's own `menus.toml`, as written.
    fn built_in() -> HashMap<String, MenuDefinition> {
        toml::from_str::<MenusFile>(include_str!("defaults/menus.toml"))
            .map_or_else(|_| HashMap::new(), |file| file.menu)
    }

    /// Returns the menus a file defines, as menus: each `builtin` item
    /// replaced by the items of the built-in menu it names, and the built-in
    /// menus the file does not define added, so a file need only say what
    /// it changes.
    fn with_built_in(mut menus: HashMap<String, MenuDefinition>) -> Result<Self, String> {
        let built_in = Self::built_in();
        for (name, menu) in &mut menus {
            let mut items = Vec::new();
            for entry in std::mem::take(&mut menu.items) {
                match entry {
                    MenuEntry::Builtin { menu: wanted } => {
                        let Some(found) = built_in.get(&wanted) else {
                            return Err(format!(
                                "[menu.{name}]: there is no built-in menu `{wanted}`"
                            ));
                        };
                        items.extend(found.items.iter().cloned());
                    }
                    MenuEntry::Command { .. }
                    | MenuEntry::Submenu { .. }
                    | MenuEntry::Toggle { .. }
                    | MenuEntry::Generate { .. }
                    | MenuEntry::Pipe { .. }
                    | MenuEntry::Separator
                    | MenuEntry::Inert { .. } => items.push(entry),
                }
            }
            menu.items = items;
        }
        for (name, menu) in built_in {
            menus.entry(name).or_insert(menu);
        }
        Ok(Self { by_name: menus })
    }
}

impl<'de> serde::Deserialize<'de> for Menus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let file = MenusFile::deserialize(deserializer)?;
        let menus = Self::with_built_in(file.menu).map_err(de::Error::custom)?;
        if let Some((menu, submenu)) = menus.missing_submenu() {
            return Err(de::Error::custom(format!(
                "[menu.{menu}] has a submenu `{submenu}`, which no [menu.{submenu}] is"
            )));
        }
        Ok(menus)
    }
}

/// One menu as written.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MenuDefinition {
    /// The title shown above the items; none shows no title.
    #[serde(default)]
    pub(crate) title: Option<String>,
    /// The items, top to bottom.
    pub(crate) items: Vec<MenuEntry>,
}

/// One item of a menu as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuEntry {
    /// Runs a command.
    Command {
        /// What the item says.
        label: String,
        /// What it runs.
        command: Command,
        /// The icon shown beside it: an icon theme's name for it, or an
        /// image file.
        icon: Option<String>,
    },
    /// Opens another menu.
    Submenu {
        /// What the item says.
        label: String,
        /// The other menu's name.
        menu: String,
        /// The icon shown beside it, as a command's.
        icon: Option<String>,
    },
    /// Toggles a state of the menu's window, with a mark showing it.
    Toggle {
        /// What the item says; the state's name when not given.
        label: Option<String>,
        /// The state.
        toggle: WindowToggle,
    },
    /// A list vaht makes itself: a submenu, or items in place.
    Generate {
        /// What the item says; the list's name when not given.
        label: Option<String>,
        /// What is listed.
        generator: MenuGenerator,
        /// Whether the list's items stand in the menu itself, rather than
        /// in a submenu.
        is_inline: bool,
        /// The icon shown beside its submenu, as a command's; none for a
        /// list in place.
        icon: Option<String>,
    },
    /// A submenu whose items a command prints, as TOML items, each time it
    /// opens (a pipe menu, as Openbox has them).
    Pipe {
        /// What the item says.
        label: String,
        /// The command line, run with the shell.
        command: String,
        /// The icon shown beside it, as a command's.
        icon: Option<String>,
    },
    /// The items of one of vaht's built-in menus, in place. Reading the file
    /// puts them there, so no such item is left in a menu read.
    Builtin {
        /// The built-in menu's name.
        menu: String,
    },
    /// A line between items.
    Separator,
    /// Text that does nothing, drawn disabled (Fluxbox's `[nop]`).
    Inert {
        /// The text.
        label: String,
    },
}

impl MenuEntry {
    /// Returns the icon the item names, if it names one; `icon = ""`, which
    /// says it has none, names nothing.
    pub(crate) fn icon(&self) -> Option<&str> {
        match self {
            Self::Command { icon, .. }
            | Self::Submenu { icon, .. }
            | Self::Pipe { icon, .. }
            | Self::Generate { icon, .. } => icon.as_deref().filter(|name| !name.is_empty()),
            Self::Toggle { .. } | Self::Builtin { .. } | Self::Separator | Self::Inert { .. } => {
                None
            }
        }
    }

    /// Returns what an item that runs a program, and says nothing of its
    /// icon, is looked up by: the program's icon. An item that says `icon =
    /// ""` has none.
    pub(crate) fn inferred_icon(&self) -> Option<InferredIcon> {
        match self {
            Self::Command {
                command,
                icon: None,
                ..
            } => InferredIcon::of(command),
            Self::Command { icon: Some(_), .. }
            | Self::Submenu { .. }
            | Self::Pipe { .. }
            | Self::Generate { .. }
            | Self::Toggle { .. }
            | Self::Builtin { .. }
            | Self::Separator
            | Self::Inert { .. } => None,
        }
    }
}

impl<'de> serde::Deserialize<'de> for MenuEntry {
    #[expect(clippy::too_many_lines, reason = "one pass over an item's forms")]
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// An item as written: one of its forms' keys, with a label.
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct EntryFile {
            /// What the item says.
            label: Option<String>,
            /// A command.
            command: Option<Command>,
            /// A submenu's name.
            submenu: Option<String>,
            /// A window state.
            toggle: Option<WindowToggle>,
            /// A generated list.
            generate: Option<MenuGenerator>,
            /// A pipe menu's command line.
            pipe: Option<String>,
            /// One of vaht's built-in menus, whose items stand here.
            builtin: Option<String>,
            /// A separator.
            #[serde(default)]
            separator: bool,
            /// Whether a generated list stands in place.
            #[serde(default)]
            inline: bool,
            /// A command's icon.
            icon: Option<String>,
        }

        let file = EntryFile::deserialize(deserializer)?;
        if file.inline && file.generate.is_none() {
            return Err(de::Error::custom("only a generated list can be inline"));
        }
        if let Some(command) = file.pipe {
            let has_other_form = file.command.is_some()
                || file.submenu.is_some()
                || file.toggle.is_some()
                || file.generate.is_some()
                || file.separator;
            return match (file.label, has_other_form) {
                (_, true) => Err(de::Error::custom(
                    "an item is one of command, submenu, pipe, toggle, generate and separator",
                )),
                (None, false) => Err(de::Error::custom("a pipe menu's item needs a label")),
                (Some(label), false) => Ok(Self::Pipe {
                    label,
                    command,
                    icon: file.icon,
                }),
            };
        }
        let opens_submenu = file.submenu.is_some() || (file.generate.is_some() && !file.inline);
        if file.icon.is_some() && file.command.is_none() && !opens_submenu {
            return Err(de::Error::custom(
                "only an item running a command or opening a submenu shows an icon",
            ));
        }
        if let Some(menu) = file.builtin {
            let is_alone = file.label.is_none()
                && file.command.is_none()
                && file.submenu.is_none()
                && file.toggle.is_none()
                && file.generate.is_none()
                && file.icon.is_none()
                && !file.separator
                && !file.inline;
            return if is_alone {
                Ok(Self::Builtin { menu })
            } else {
                Err(de::Error::custom("`builtin` is the whole of its item"))
            };
        }
        let entry = match (
            file.command,
            file.submenu,
            file.toggle,
            file.generate,
            file.separator,
        ) {
            (Some(command), None, None, None, false) => {
                let label = match (file.label, &command) {
                    (Some(label), _) => label,
                    (None, Command::Run(action)) => action.to_string(),
                    (None, _) => {
                        return Err(de::Error::custom(
                            "an item running several actions needs a label",
                        ));
                    }
                };
                Self::Command {
                    label,
                    command,
                    icon: file.icon,
                }
            }
            (None, Some(menu), None, None, false) => Self::Submenu {
                label: file.label.unwrap_or_else(|| menu.clone()),
                menu,
                icon: file.icon,
            },
            (None, None, Some(toggle), None, false) => Self::Toggle {
                label: file.label,
                toggle,
            },
            (None, None, None, Some(generator), false) => Self::Generate {
                label: file.label,
                generator,
                is_inline: file.inline,
                icon: file.icon,
            },
            (None, None, None, None, true) if file.label.is_none() => Self::Separator,
            (None, None, None, None, false) => match file.label {
                Some(label) => Self::Inert { label },
                None => return Err(de::Error::custom("an item needs a label, or what it does")),
            },
            _ => {
                return Err(de::Error::custom(
                    "an item is one of command, submenu, toggle, generate and separator",
                ));
            }
        };
        Ok(entry)
    }
}

/// What an item that runs a program is looked up by when it names no icon:
/// the names to try, best first, for the program's icon.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct InferredIcon(Vec<String>);

impl InferredIcon {
    /// Returns what `command` is looked up by, if it runs a program: `exec`
    /// the program of its command line, and `focus-or-exec` the application
    /// id, then that program.
    pub(crate) fn of(command: &Command) -> Option<Self> {
        let Command::Run(action) = command else {
            return None;
        };
        let names: Vec<String> = if let Action::Exec(command_line) = action {
            program_of(command_line).into_iter().collect()
        } else if let Action::FocusOrExec {
            app_id,
            command_line,
        } = action
        {
            std::iter::once(app_id.clone())
                .chain(program_of(command_line))
                .collect()
        } else {
            return None;
        };
        (!names.is_empty()).then_some(Self(names))
    }

    /// Returns the names to look the icon up by, best first.
    pub(crate) fn names(&self) -> &[String] {
        &self.0
    }

    /// Returns the name an icon found for this is kept under, which no icon a
    /// menu names itself is.
    pub(crate) fn key(&self) -> String {
        format!("app:{}", self.0.join("|"))
    }
}

/// The shells and wrappers whose command line says nothing of what it runs.
const OPAQUE_PROGRAMS: [&str; 8] = ["sh", "bash", "zsh", "fish", "dash", "env", "sudo", "setsid"];

/// Returns the name of the program `command_line` runs: its first word that is
/// not a `NAME=value` setting or the `exec` word, quotes taken off and the
/// directory dropped; none if that is a shell or a wrapper, or there is none.
fn program_of(command_line: &str) -> Option<String> {
    let word = command_line.split_whitespace().find(|word| {
        let is_setting = word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|letter| letter.is_alphanumeric() || letter == '_')
        });
        !is_setting && *word != "exec"
    })?;
    let word = word.trim_matches(['"', '\'']);
    let program = word.rsplit('/').next().unwrap_or(word);
    (!program.is_empty() && !program.starts_with('-') && !OPAQUE_PROGRAMS.contains(&program))
        .then(|| program.to_owned())
}

/// A window state a menu item toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WindowToggle {
    /// Rolled up into the titlebar.
    Shade,
    /// On every workspace.
    Stick,
    /// Showing only the wallpaper through its transparent parts.
    PseudoTransparent,
    /// Maximized.
    Maximize,
    /// Minimized.
    Minimize,
    /// Covering the whole output.
    Fullscreen,
}

impl WindowToggle {
    /// Returns what the item says when its label is not given.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Shade => "Shade",
            Self::Stick => "Stick",
            Self::PseudoTransparent => "Pseudo-Transparent",
            Self::Maximize => "Maximize",
            Self::Minimize => "Minimize",
            Self::Fullscreen => "Fullscreen",
        }
    }

    /// Returns the action that toggles the state.
    pub(crate) const fn action(self) -> WindowManagerAction {
        match self {
            Self::Shade => WindowManagerAction::Shade,
            Self::Stick => WindowManagerAction::Stick,
            Self::PseudoTransparent => WindowManagerAction::PseudoTransparent,
            Self::Maximize => WindowManagerAction::Maximize(Axes::Both),
            Self::Minimize => WindowManagerAction::Minimize,
            Self::Fullscreen => WindowManagerAction::Fullscreen,
        }
    }

    /// Returns the command that toggles the state.
    pub(crate) const fn command(self) -> Command {
        Command::Run(Action::WindowManager(self.action()))
    }
}

/// A list a menu item opens, which vaht makes as the menu opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum MenuGenerator {
    /// The workspaces: choosing one shows it.
    Workspaces,
    /// The workspaces: choosing one sends the menu's window there.
    SendToWorkspace,
    /// The windows: choosing one brings it forward.
    WindowList,
    /// Opacities for the menu's window: opaque, 90%, 75%, 50%, and the
    /// settings' (fluxland's `[alpha]` steps).
    Alpha,
    /// The stacking layers, the menu's window's own checked: choosing one
    /// moves the window there.
    Layer,
    /// The styles there are, the one in use checked: choosing one makes it
    /// the style.
    Styles,
    /// The layouts there are, the one in use checked: choosing one makes it
    /// the layout.
    Layouts,
    /// The options the style in use declares, a submenu of each listing its
    /// choices with the one in effect checked: choosing one makes it the
    /// choice.
    StyleOptions,
    /// The edges the taskbar can be on, the one it is on checked: choosing
    /// one moves it.
    Positions,
    /// The display profiles, the one applied checked: choosing one
    /// applies it.
    Profiles,
    /// The outputs on, the primary one checked: choosing one makes it
    /// primary.
    Outputs,
    /// vaht's own menu: the styles, their options, the layouts and the
    /// taskbar's position, then settings, the command dialog, reload and
    /// save, and exit. It is the same wherever it is put.
    Vaht,
}

impl MenuGenerator {
    /// The generators by name.
    const NAMES: [(&str, Self); 12] = [
        ("workspaces", Self::Workspaces),
        ("send-to-workspace", Self::SendToWorkspace),
        ("window-list", Self::WindowList),
        ("alpha", Self::Alpha),
        ("layer", Self::Layer),
        ("styles", Self::Styles),
        ("layouts", Self::Layouts),
        ("style-options", Self::StyleOptions),
        ("positions", Self::Positions),
        ("profiles", Self::Profiles),
        ("outputs", Self::Outputs),
        ("vaht", Self::Vaht),
    ];

    /// Returns what the item says when its label is not given.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Workspaces => "Workspaces",
            Self::SendToWorkspace => "Send To...",
            Self::WindowList => "Windows",
            Self::Alpha => "Transparency",
            Self::Layer => "Layer",
            Self::Styles => "Styles",
            Self::Layouts => "Layouts",
            Self::StyleOptions => "Style Options",
            Self::Positions => "Position",
            Self::Profiles => "Display Profiles",
            Self::Outputs => "Primary Output",
            Self::Vaht => "vaht",
        }
    }
}

impl<'de> serde::Deserialize<'de> for MenuGenerator {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::NAMES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|&(_, generator)| generator)
            .ok_or_else(|| {
                de::Error::custom(format!(
                    "`{name}` is no list; they are workspaces, send-to-workspace, window-list, alpha, layer, styles, layouts, style-options, positions, profiles, outputs and vaht"
                ))
            })
    }
}

impl fmt::Display for MenuGenerator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = Self::NAMES
            .iter()
            .find(|(_, generator)| generator == self)
            .map_or("", |(name, _)| name);
        formatter.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Reads `text` as `menus.toml`.
    fn read(text: &str) -> Result<Menus, toml::de::Error> {
        toml::from_str(text)
    }

    #[test]
    fn the_built_in_menus_read() {
        let menus = read(include_str!("defaults/menus.toml")).expect("valid");
        for name in [Menus::ROOT, Menus::WINDOW, Menus::WORKSPACE] {
            assert!(menus.get(name).is_some(), "{name}");
        }
    }

    #[rstest]
    #[case::plain("kitty", Some("kitty"))]
    #[case::with_arguments("kitty --single-instance", Some("kitty"))]
    #[case::a_path("/usr/bin/brave --incognito", Some("brave"))]
    #[case::settings_first("GDK_BACKEND=x11 FOO=1 steam -silent", Some("steam"))]
    #[case::the_exec_word("exec xterm", Some("xterm"))]
    #[case::quoted("\"spotify-launcher\"", Some("spotify-launcher"))]
    #[case::a_shell("sh -c 'something'", None)]
    #[case::a_wrapper("env FOO=1", None)]
    #[case::nothing("", None)]
    #[case::only_settings("FOO=1", None)]
    #[case::an_option("--help", None)]
    fn the_program_a_command_line_runs_is_named(
        #[case] line: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(program_of(line).as_deref(), expected, "{line}");
    }

    #[test]
    fn an_icon_is_looked_up_for_the_programs_an_item_runs_unless_it_says_otherwise() {
        let menus = read(
            "[menu.root]\nitems = [\n\
             { label = \"a\", command = \"exec kitty --x\" },\n\
             { label = \"b\", command = \"focus-or-exec Spotify spotify-launcher\" },\n\
             { label = \"c\", command = \"exec xterm\", icon = \"\" },\n\
             { label = \"d\", command = \"exec xterm\", icon = \"named\" },\n\
             { label = \"e\", command = \"reload\" },\n\
             { label = \"f\", generate = \"styles\", icon = \"palette\" },\n]\n",
        )
        .expect("valid");
        let items = &menus.get("root").expect("read").items;
        let names = |index: usize| {
            items[index]
                .inferred_icon()
                .map(|icon| icon.names().to_vec())
        };
        assert_eq!(names(0), Some(vec!["kitty".to_owned()]));
        assert_eq!(
            names(1),
            Some(vec!["Spotify".to_owned(), "spotify-launcher".to_owned()])
        );
        assert_eq!(names(2), None, "it says it has none");
        assert_eq!(names(3), None, "it names one");
        assert_eq!(names(4), None, "it runs no program");
        assert_eq!(items[2].icon(), None, "an empty name is no icon");
        assert_eq!(items[3].icon(), Some("named"));
        assert_eq!(
            items[5].icon(),
            Some("palette"),
            "a list's submenu may have one"
        );
        assert_eq!(menus.icons(), ["named", "palette"]);
        assert_eq!(menus.inferred_icons().len(), 2);
    }

    #[test]
    fn an_icon_on_a_list_in_place_is_refused() {
        let error =
            read("[menu.root]\nitems = [{ generate = \"styles\", inline = true, icon = \"x\" }]\n")
                .expect_err("an inline list has no item to show it");
        assert!(error.to_string().contains("only an item"), "{error}");
    }

    #[test]
    fn a_menu_the_file_leaves_out_is_the_built_in_one() {
        let menus =
            read("[menu.root]\nitems = [{ label = \"kitty\", command = \"exec kitty\" }]\n")
                .expect("valid");
        assert_eq!(menus.get("root").map(|menu| menu.items.len()), Some(1));
        let built_in = read(include_str!("defaults/menus.toml")).expect("valid");
        for name in [Menus::WINDOW, Menus::WORKSPACE] {
            assert_eq!(menus.get(name), built_in.get(name), "{name}");
        }
        // One the file defines is its own, even empty.
        let own = read("[menu.window]\nitems = []\n").expect("valid");
        assert_eq!(own.get(Menus::WINDOW).map(|menu| menu.items.len()), Some(0));
    }

    #[test]
    fn a_builtin_item_puts_the_built_in_menus_items_in_place() {
        let menus = read(
            "[menu.window]\nitems = [{ label = \"Mine\", command = \"reload\" }, \
             { separator = true }, { builtin = \"window\" }]\n",
        )
        .expect("valid");
        let built_in = read(include_str!("defaults/menus.toml")).expect("valid");
        let built_in = &built_in.get(Menus::WINDOW).expect("built in").items;
        let items = &menus.get(Menus::WINDOW).expect("read").items;
        assert_eq!(items.len(), built_in.len() + 2);
        assert_eq!(&items[2..], built_in.as_slice());
    }

    #[rstest]
    #[case::unknown(
        "[menu.root]\nitems = [{ builtin = \"nothing\" }]\n",
        "no built-in menu"
    )]
    #[case::with_more(
        "[menu.root]\nitems = [{ builtin = \"window\", label = \"x\" }]\n",
        "whole of its item"
    )]
    fn a_builtin_item_must_name_a_built_in_menu_alone(#[case] text: &str, #[case] expected: &str) {
        let error = read(text).expect_err(text);
        assert!(error.to_string().contains(expected), "{error}");
    }

    #[test]
    fn every_item_form_reads() {
        let menus = read(
            "[menu.root]\ntitle = \"vaht\"\nitems = [\n\
             { label = \"kitty\", command = \"exec kitty\" },\n\
             { label = \"More\", submenu = \"more\" },\n\
             { toggle = \"shade\" },\n\
             { generate = \"workspaces\" },\n\
             { separator = true },\n\
             { label = \"just words\" },\n\
             { command = \"reload\" },\n]\n\
             [menu.more]\nitems = []\n",
        )
        .expect("valid");
        let root = menus.get("root").expect("read");
        assert_eq!(root.title.as_deref(), Some("vaht"));
        assert_eq!(
            root.items,
            [
                MenuEntry::Command {
                    label: "kitty".to_owned(),
                    command: Command::Run(Action::Exec("kitty".to_owned())),
                    icon: None,
                },
                MenuEntry::Submenu {
                    label: "More".to_owned(),
                    menu: "more".to_owned(),
                    icon: None,
                },
                MenuEntry::Toggle {
                    label: None,
                    toggle: WindowToggle::Shade
                },
                MenuEntry::Generate {
                    label: None,
                    generator: MenuGenerator::Workspaces,
                    is_inline: false,
                    icon: None,
                },
                MenuEntry::Separator,
                MenuEntry::Inert {
                    label: "just words".to_owned()
                },
                MenuEntry::Command {
                    label: "reload".to_owned(),
                    command: Command::Run(Action::Reload),
                    icon: None,
                },
            ]
        );
    }

    #[rstest]
    #[case::missing_submenu(
        "[menu.root]\nitems = [{ submenu = \"nowhere\" }]\n",
        "no [menu.nowhere]"
    )]
    #[case::two_forms(
        "[menu.root]\nitems = [{ command = \"reload\", submenu = \"root\" }]\n",
        "one of command"
    )]
    #[case::unknown_list("[menu.root]\nitems = [{ generate = \"pets\" }]\n", "no list")]
    #[case::empty_item("[menu.root]\nitems = [{}]\n", "needs a label")]
    #[case::icon_without_a_command(
        "[menu.root]\nitems = [{ separator = true, icon = \"x\" }]\n",
        "only an item running a command or opening"
    )]
    fn mistakes_are_explained(#[case] text: &str, #[case] expected: &str) {
        let error = read(text).expect_err(text);
        assert!(error.message().contains(expected), "{error}");
    }
}

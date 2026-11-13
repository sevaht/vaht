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
                | MenuEntry::Toggle { .. }
                | MenuEntry::Generate { .. }
                | MenuEntry::Separator
                | MenuEntry::Inert { .. } => None,
            })
        })
    }
}

impl<'de> serde::Deserialize<'de> for Menus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The file as written: `[menu.<name>]` tables.
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct MenusFile {
            /// The menus.
            #[serde(default)]
            menu: HashMap<String, MenuDefinition>,
        }

        let file = MenusFile::deserialize(deserializer)?;
        let menus = Self { by_name: file.menu };
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
    },
    /// Opens another menu.
    Submenu {
        /// What the item says.
        label: String,
        /// The other menu's name.
        menu: String,
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
    },
    /// A line between items.
    Separator,
    /// Text that does nothing, drawn disabled (Fluxbox's `[nop]`).
    Inert {
        /// The text.
        label: String,
    },
}

impl<'de> serde::Deserialize<'de> for MenuEntry {
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
            /// A separator.
            #[serde(default)]
            separator: bool,
            /// Whether a generated list stands in place.
            #[serde(default)]
            inline: bool,
        }

        let file = EntryFile::deserialize(deserializer)?;
        if file.inline && file.generate.is_none() {
            return Err(de::Error::custom("only a generated list can be inline"));
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
                    (None, Command::Sequence(_) | Command::Repeat { .. }) => {
                        return Err(de::Error::custom(
                            "an item running several actions needs a label",
                        ));
                    }
                };
                Self::Command { label, command }
            }
            (None, Some(menu), None, None, false) => Self::Submenu {
                label: file.label.unwrap_or_else(|| menu.clone()),
                menu,
            },
            (None, None, Some(toggle), None, false) => Self::Toggle {
                label: file.label,
                toggle,
            },
            (None, None, None, Some(generator), false) => Self::Generate {
                label: file.label,
                generator,
                is_inline: file.inline,
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

/// A window state a menu item toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WindowToggle {
    /// Rolled up into the titlebar.
    Shade,
    /// On every workspace.
    Stick,
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
}

impl MenuGenerator {
    /// The generators by name.
    const NAMES: [(&str, Self); 4] = [
        ("workspaces", Self::Workspaces),
        ("send-to-workspace", Self::SendToWorkspace),
        ("window-list", Self::WindowList),
        ("alpha", Self::Alpha),
    ];

    /// Generators of 02 §8 that vaht does not have yet.
    const PLANNED: [&str; 4] = ["themes", "layer", "outputs", "profiles"];

    /// Returns what the item says when its label is not given.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Workspaces => "Workspaces",
            Self::SendToWorkspace => "Send To...",
            Self::WindowList => "Windows",
            Self::Alpha => "Transparency",
        }
    }
}

impl<'de> serde::Deserialize<'de> for MenuGenerator {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        if Self::PLANNED.contains(&name.as_str()) {
            return Err(de::Error::custom(format!(
                "the `{name}` list is not available in vaht yet"
            )));
        }
        Self::NAMES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|&(_, generator)| generator)
            .ok_or_else(|| {
                de::Error::custom(format!(
                    "`{name}` is no list; they are workspaces, send-to-workspace, window-list and alpha"
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
                    command: Command::Run(Action::Exec("kitty".to_owned()))
                },
                MenuEntry::Submenu {
                    label: "More".to_owned(),
                    menu: "more".to_owned()
                },
                MenuEntry::Toggle {
                    label: None,
                    toggle: WindowToggle::Shade
                },
                MenuEntry::Generate {
                    label: None,
                    generator: MenuGenerator::Workspaces,
                    is_inline: false,
                },
                MenuEntry::Separator,
                MenuEntry::Inert {
                    label: "just words".to_owned()
                },
                MenuEntry::Command {
                    label: "reload".to_owned(),
                    command: Command::Run(Action::Reload)
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
    #[case::planned_list("[menu.root]\nitems = [{ generate = \"themes\" }]\n", "not available")]
    #[case::unknown_list("[menu.root]\nitems = [{ generate = \"pets\" }]\n", "no list")]
    #[case::empty_item("[menu.root]\nitems = [{}]\n", "needs a label")]
    fn mistakes_are_explained(#[case] text: &str, #[case] expected: &str) {
        let error = read(text).expect_err(text);
        assert!(error.message().contains(expected), "{error}");
    }
}

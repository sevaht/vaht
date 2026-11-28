//! What a menu shows, whoever shows it (01 §6.4).
//!
//! A [`MenuModel`] is a value: a title and items, each with its label, what
//! choosing it does ([`MenuChoice`]) or the submenu it opens, and its mark
//! (a toggle's check). [`MenuBuilder`] makes one from a menu of
//! `menus.toml` and what the window manager knows as the menu opens: toggles
//! read the window's state, and generated lists read the workspaces and
//! windows. The model knows nothing of how it is drawn.

use std::collections::{BTreeMap, HashMap};

use crate::config::options::ListedOption;
use crate::config::{
    Action, Command, MenuDefinition, MenuEntry, MenuGenerator, Menus, SettingChange,
    WindowManagerAction, WindowToggle,
};
use crate::tray::{TrayItemKey, TrayMenu, TrayMenuEntry};
use crate::window_manager::{AlphaChange, StackLayer, WindowId, WindowManager, WorkspaceTarget};

/// A menu: its title and items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuModel {
    /// The title shown above the items, if any.
    pub(crate) title: Option<String>,
    /// The items, top to bottom.
    pub(crate) items: Vec<MenuItem>,
}

/// One item of a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuItem {
    /// An item that does something when chosen.
    Choice {
        /// What it says.
        label: String,
        /// What choosing it does.
        choice: MenuChoice,
        /// Its toggle's mark, if it toggles something.
        mark: Option<bool>,
        /// Its icon, if it shows one: a window's, in a list of windows.
        icon: Option<vaht_ui::Icon>,
    },
    /// An item that opens another menu.
    Submenu {
        /// What it says.
        label: String,
        /// The menu it opens.
        menu: MenuModel,
        /// Its icon, if it shows one.
        icon: Option<vaht_ui::Icon>,
        /// For a pipe menu, the command line whose output fills it when it
        /// opens; `menu` waits for it meanwhile.
        pipe: Option<String>,
    },
    /// Text that does nothing.
    Inert {
        /// What it says.
        label: String,
    },
    /// A line between items.
    Separator,
}

impl MenuModel {
    /// Returns the tray item's `menu` as a model: an entry that cannot be
    /// chosen is inert text.
    pub(crate) fn of_tray_menu(menu: &TrayMenu) -> Self {
        Self {
            title: None,
            items: Self::tray_items(menu.item, &menu.entries),
        }
    }

    /// Returns `entries`, of the tray item `item`'s menu, as items.
    fn tray_items(item: TrayItemKey, entries: &[TrayMenuEntry]) -> Vec<MenuItem> {
        entries
            .iter()
            .map(|entry| match entry {
                TrayMenuEntry::Choice {
                    id,
                    label,
                    is_enabled: true,
                    mark,
                } => MenuItem::Choice {
                    label: label.clone(),
                    choice: MenuChoice::TrayMenuEntry { item, id: *id },
                    mark: *mark,
                    icon: None,
                },
                TrayMenuEntry::Choice {
                    label,
                    is_enabled: false,
                    ..
                } => MenuItem::Inert {
                    label: label.clone(),
                },
                TrayMenuEntry::Submenu { label, entries } => MenuItem::Submenu {
                    label: label.clone(),
                    menu: Self {
                        title: None,
                        items: Self::tray_items(item, entries),
                    },
                    icon: None,
                    pipe: None,
                },
                TrayMenuEntry::Separator => MenuItem::Separator,
            })
            .collect()
    }
}

impl MenuItem {
    /// Returns what the item says; nothing for a separator.
    pub(crate) fn label(&self) -> Option<&str> {
        match self {
            Self::Choice { label, .. } | Self::Submenu { label, .. } | Self::Inert { label } => {
                Some(label)
            }
            Self::Separator => None,
        }
    }

    /// Returns whether the item shows an icon.
    pub(crate) const fn has_icon(&self) -> bool {
        matches!(
            self,
            Self::Choice { icon: Some(_), .. } | Self::Submenu { icon: Some(_), .. }
        )
    }

    /// Returns whether the item can be highlighted and chosen.
    pub(crate) const fn is_selectable(&self) -> bool {
        matches!(self, Self::Choice { .. } | Self::Submenu { .. })
    }
}

/// What choosing an item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuChoice {
    /// Runs a command on a window (the menu's), or on none.
    Run {
        /// The command.
        command: Command,
        /// The window it acts on.
        window: Option<WindowId>,
    },
    /// Brings a window forward: shows its workspace, restores it, focuses
    /// and raises it.
    Activate(WindowId),
    /// Tells a tray item that an entry of its menu was chosen.
    TrayMenuEntry {
        /// The tray item.
        item: TrayItemKey,
        /// The item's id for the entry.
        id: i32,
    },
    /// Answers an IPC client's pick with item `index` of its list.
    AnswerPick(usize),
    /// Answers whether an X program (its emulating client `client`) may
    /// emulate input.
    AnswerInputEmulation {
        /// The client asked about.
        client: u64,
        /// Whether it may.
        is_allowed: bool,
        /// The program, if the answer is for it from now on, not just
        /// until vaht exits.
        from_now_on: Option<String>,
    },
}

impl MenuChoice {
    /// Returns whether choosing leaves the menus open, as Fluxbox's style
    /// menus do, so one style or layout can be tried after another.
    pub(crate) fn keeps_menus_open(&self) -> bool {
        matches!(
            self,
            Self::Run {
                command: Command::Run(Action::SetStyle(_) | Action::SetLayout(_)),
                ..
            }
        ) || matches!(
            self,
            Self::Run {
                command: Command::Run(Action::SetSetting(change)),
                ..
            } if change.is_style_option()
        )
    }
}

/// Makes menus from `menus.toml` and the window manager's state.
#[derive(Debug, Clone)]
pub(crate) struct MenuBuilder<'state> {
    /// The menus as written.
    menus: &'state Menus,
    /// The windows and workspaces.
    window_manager: &'state WindowManager,
    /// The window the menu is about (the window menu's), if any.
    window: Option<WindowId>,
    /// The styles there are, for the `styles` list.
    styles: Vec<String>,
    /// The style in use.
    current_style: Option<&'state str>,
    /// The layouts there are, for the `layouts` list.
    layouts: Vec<String>,
    /// The layout in use, if there is one.
    current_layout: Option<&'state str>,
    /// The edge the taskbar is on, as the setting names it.
    taskbar_edge: &'static str,
    /// The options of the style in use, for the `style-options` list.
    style_options: Vec<ListedOption>,
    /// Each window's icon, for lists of windows that show them.
    window_icons: HashMap<WindowId, vaht_ui::Icon>,
    /// The icons `menus.toml` names, by the name written, those found.
    named_icons: HashMap<String, vaht_ui::Icon>,
    /// The profiles and outputs, for the `profiles` and `outputs` lists.
    outputs: OutputChoices,
}

/// The display profiles and outputs there are to choose from.
#[derive(Debug, Clone, Default)]
pub(crate) struct OutputChoices {
    /// The profiles' names, in the order written.
    pub(crate) profiles: Vec<String>,
    /// The profile applied, if one is.
    pub(crate) profile: Option<String>,
    /// The outputs on, by connector, the primary one first.
    pub(crate) outputs: Vec<String>,
}

impl<'state> MenuBuilder<'state> {
    /// How deep submenus go before vaht stops following them: a menu that
    /// is its own submenu goes no deeper.
    const MAX_DEPTH: usize = 8;

    /// The `alpha` list's items: fluxland's steps, and back to the
    /// settings'.
    const ALPHA_STEPS: [(&str, AlphaChange); 5] = [
        ("Opaque", AlphaChange::both(255)),
        ("90%", AlphaChange::both(230)),
        ("75%", AlphaChange::both(191)),
        ("50%", AlphaChange::both(128)),
        ("Use Defaults", AlphaChange::Reset),
    ];

    /// A builder of menus about `window`.
    pub(crate) fn new(
        menus: &'state Menus,
        window_manager: &'state WindowManager,
        window: Option<WindowId>,
    ) -> Self {
        Self {
            menus,
            window_manager,
            window,
            styles: Vec::new(),
            current_style: None,
            layouts: Vec::new(),
            current_layout: None,
            taskbar_edge: "bottom",
            style_options: Vec::new(),
            window_icons: HashMap::new(),
            named_icons: HashMap::new(),
            outputs: OutputChoices::default(),
        }
    }

    /// Returns the builder listing the profiles and outputs of `choices`.
    #[must_use]
    pub(crate) fn with_outputs(mut self, choices: OutputChoices) -> Self {
        self.outputs = choices;
        self
    }

    /// Returns the builder showing `icons`, by the name `menus.toml`
    /// writes, beside the items naming them.
    pub(crate) fn with_named_icons(mut self, icons: HashMap<String, vaht_ui::Icon>) -> Self {
        self.named_icons = icons;
        self
    }

    /// Returns the builder showing `icons` beside the windows they belong
    /// to in lists of windows (Fluxbox's `clientMenu.usePixmap`).
    #[must_use]
    pub(crate) fn with_window_icons(mut self, icons: HashMap<WindowId, vaht_ui::Icon>) -> Self {
        self.window_icons = icons;
        self
    }

    /// Returns the builder listing `styles` in the `styles` list, `current`
    /// checked.
    #[must_use]
    pub(crate) fn with_styles(mut self, styles: Vec<String>, current: Option<&'state str>) -> Self {
        self.styles = styles;
        self.current_style = current;
        self
    }

    /// Returns the builder listing `layouts` in the `layouts` list, `current`
    /// checked.
    #[must_use]
    pub(crate) fn with_layouts(
        mut self,
        layouts: Vec<String>,
        current: Option<&'state str>,
    ) -> Self {
        self.layouts = layouts;
        self.current_layout = current;
        self
    }

    /// Returns the builder listing `options` in the `style-options` list.
    #[must_use]
    pub(crate) fn with_style_options(mut self, options: Vec<ListedOption>) -> Self {
        self.style_options = options;
        self
    }

    /// Returns the builder with the taskbar on `edge` (`bottom` or `top`),
    /// for the `positions` list.
    #[must_use]
    pub(crate) const fn with_taskbar_edge(mut self, edge: &'static str) -> Self {
        self.taskbar_edge = edge;
        self
    }

    /// Returns the menu named `name`, if `menus.toml` has one.
    pub(crate) fn build(&self, name: &str) -> Option<MenuModel> {
        self.menus
            .get(name)
            .map(|definition| self.build_definition(definition, 0))
    }

    /// Returns the list `generator` makes, titled `title`.
    pub(crate) fn generated(&self, generator: MenuGenerator, title: Option<String>) -> MenuModel {
        MenuModel {
            title,
            items: self.generate(generator),
        }
    }

    /// Returns what a pipe menu shows while its command runs.
    pub(crate) fn waiting() -> MenuModel {
        MenuModel {
            title: None,
            items: vec![MenuItem::Inert {
                label: "…".to_owned(),
            }],
        }
    }

    /// Returns `definition`, which a pipe menu's command printed, as a
    /// model.
    pub(crate) fn build_printed(&self, definition: &MenuDefinition) -> MenuModel {
        self.build_definition(definition, 0)
    }

    /// Returns `definition` as a model, its submenus `depth` deep.
    fn build_definition(&self, definition: &MenuDefinition, depth: usize) -> MenuModel {
        let mut items = Vec::new();
        for entry in &definition.items {
            match entry {
                MenuEntry::Command {
                    label,
                    command,
                    icon,
                } => items.push(MenuItem::Choice {
                    label: label.clone(),
                    choice: MenuChoice::Run {
                        command: command.clone(),
                        window: self.window,
                    },
                    mark: None,
                    icon: icon
                        .as_ref()
                        .and_then(|name| self.named_icons.get(name).cloned()),
                }),
                MenuEntry::Submenu { label, menu, icon } => {
                    if depth >= Self::MAX_DEPTH {
                        tracing::warn!(menu, "menus nest too deep; not following");
                        continue;
                    }
                    if let Some(submenu) = self.menus.get(menu) {
                        items.push(MenuItem::Submenu {
                            label: label.clone(),
                            menu: self.build_definition(submenu, depth + 1),
                            icon: icon
                                .as_ref()
                                .and_then(|name| self.named_icons.get(name).cloned()),
                            pipe: None,
                        });
                    }
                }
                MenuEntry::Pipe {
                    label,
                    command,
                    icon,
                } => items.push(MenuItem::Submenu {
                    label: label.clone(),
                    menu: Self::waiting(),
                    icon: icon
                        .as_ref()
                        .and_then(|name| self.named_icons.get(name).cloned()),
                    pipe: Some(command.clone()),
                }),
                MenuEntry::Toggle { label, toggle } => {
                    // A toggle is about a window; without one it is left out.
                    if let Some(item) = self.toggle(*toggle, label.as_deref()) {
                        items.push(item);
                    }
                }
                MenuEntry::Generate {
                    label,
                    generator,
                    is_inline,
                } => {
                    let generated = self.generate(*generator);
                    if *is_inline {
                        items.extend(generated);
                    } else {
                        // Titled with its item's label, as a submenu of its own
                        // definition is.
                        let label = label
                            .clone()
                            .unwrap_or_else(|| generator.label().to_owned());
                        items.push(MenuItem::Submenu {
                            menu: MenuModel {
                                title: Some(label.clone()),
                                items: generated,
                            },
                            label,
                            icon: None,
                            pipe: None,
                        });
                    }
                }
                // Reading the file replaced these with the menu's items.
                MenuEntry::Builtin { .. } => {}
                MenuEntry::Separator => items.push(MenuItem::Separator),
                MenuEntry::Inert { label } => items.push(MenuItem::Inert {
                    label: label.clone(),
                }),
            }
        }
        MenuModel {
            title: definition.title.clone(),
            items,
        }
    }

    /// Returns the item toggling `toggle` of the menu's window, marked as
    /// the window is, if the menu is about a window.
    fn toggle(&self, toggle: WindowToggle, label: Option<&str>) -> Option<MenuItem> {
        let window = self.window_manager.window(self.window?)?;
        let state = window.state();
        let is_on = match toggle {
            WindowToggle::Shade => state.is_shaded,
            WindowToggle::Stick => state.is_sticky,
            WindowToggle::PseudoTransparent => window.is_pseudo_transparent(),
            WindowToggle::Maximize => window.placement().is_fully_maximized(),
            WindowToggle::Minimize => state.is_minimized,
            WindowToggle::Fullscreen => window.placement().is_fullscreen(),
        };
        Some(MenuItem::Choice {
            label: label.unwrap_or(toggle.label()).to_owned(),
            choice: MenuChoice::Run {
                command: toggle.command(),
                window: self.window,
            },
            mark: Some(is_on),
            icon: None,
        })
    }

    /// Returns the choice of `option` that setting it to `choice` makes.
    fn option_choice(option: &ListedOption, choice: &str) -> MenuChoice {
        MenuChoice::Run {
            command: Command::Run(Action::SetSetting(SettingChange::style_option(
                &option.name,
                choice,
            ))),
            window: None,
        }
    }

    /// Returns a submenu titled `title` of `items`.
    fn submenu_of(title: &str, items: Vec<MenuItem>) -> MenuItem {
        MenuItem::Submenu {
            label: title.to_owned(),
            menu: MenuModel {
                title: Some(title.to_owned()),
                items,
            },
            icon: None,
            pipe: None,
        }
    }

    /// Returns `option` as a checkbox, if it says what to call one and has
    /// two choices: checked when the one that is not the default is chosen.
    fn option_checkbox(option: &ListedOption) -> Option<MenuItem> {
        let label = option.checkbox.clone()?;
        let [first, second] = option.choices.as_slice() else {
            return None;
        };
        // Checked is the choice the style names, else the one that is not the
        // default.
        let checked = option.checked.as_ref().unwrap_or_else(|| {
            if *first == option.default {
                second
            } else {
                first
            }
        });
        let unchecked = if checked == first { second } else { first };
        let is_checked = option.chosen == *checked;
        Some(MenuItem::Choice {
            label,
            choice: Self::option_choice(option, if is_checked { unchecked } else { checked }),
            mark: Some(is_checked),
            icon: None,
        })
    }

    /// Returns `option` as a submenu of its choices, the one in effect
    /// checked: the default first, a line under it, then the choices in no
    /// group, then a submenu for each group.
    fn option_submenu(option: &ListedOption) -> MenuItem {
        let item_of = |choice: &String| MenuItem::Choice {
            label: vaht_ipc::choice_title(choice),
            choice: Self::option_choice(option, choice),
            mark: Some(*choice == option.chosen),
            icon: None,
        };
        let mut choices: Vec<MenuItem> = Vec::new();
        let mut grouped: BTreeMap<&str, Vec<MenuItem>> = BTreeMap::new();
        for choice in &option.choices {
            match option.groups.get(choice) {
                Some(group) if *choice != option.default => {
                    grouped.entry(group).or_default().push(item_of(choice));
                }
                _ => choices.push(item_of(choice)),
            }
        }
        choices.extend(
            grouped
                .into_iter()
                .map(|(group, items)| Self::submenu_of(group, items)),
        );
        if choices.len() > 1 {
            choices.insert(1, MenuItem::Separator);
        }
        Self::submenu_of(option.title(), choices)
    }

    /// Puts `item` in the group called `group`, which is made last if new.
    fn add_to_group<'name>(
        grouped: &mut Vec<(&'name str, Vec<MenuItem>)>,
        group: &'name str,
        item: MenuItem,
    ) {
        if let Some((_, items)) = grouped.iter_mut().find(|(known, _)| *known == group) {
            items.push(item);
        } else {
            grouped.push((group, vec![item]));
        }
    }

    /// Returns the `style-options` list: a checkbox for each option that has
    /// two choices and a label for it and no group (each its own, so checking
    /// one never changes another), then a submenu for each of the other
    /// options, and one for each group, which holds its options (checkboxes
    /// among them), then an item putting every option back to its default.
    fn style_option_items(options: &[ListedOption]) -> Vec<MenuItem> {
        if options.is_empty() {
            return vec![MenuItem::Inert {
                label: "(this style has no options)".to_owned(),
            }];
        }
        let mut items = Vec::new();
        let mut plain: Vec<MenuItem> = Vec::new();
        // The groups in the order their first options come in.
        let mut grouped: Vec<(&str, Vec<MenuItem>)> = Vec::new();
        for option in options {
            // A checkbox is in its group's submenu, if it has a group, else
            // loose at the top.
            match (Self::option_checkbox(option), &option.group) {
                (Some(item), None) => items.push(item),
                (Some(item), Some(group)) => Self::add_to_group(&mut grouped, group, item),
                (None, Some(group)) => {
                    Self::add_to_group(&mut grouped, group, Self::option_submenu(option));
                }
                (None, None) => plain.push(Self::option_submenu(option)),
            }
        }
        plain.extend(
            grouped
                .into_iter()
                .map(|(group, submenus)| Self::submenu_of(group, submenus)),
        );
        if !items.is_empty() && !plain.is_empty() {
            items.push(MenuItem::Separator);
        }
        items.extend(plain);
        items.push(MenuItem::Separator);
        items.push(MenuItem::Choice {
            label: "Reset all to defaults".to_owned(),
            choice: MenuChoice::Run {
                command: Command::Run(Action::SetSetting(SettingChange::unset(vec![
                    "style_options".to_owned(),
                ]))),
                window: None,
            },
            mark: None,
            icon: None,
        });
        items
    }

    /// Returns the item running `action`, labeled `label`.
    fn action_item(label: &str, action: Action) -> MenuItem {
        MenuItem::Choice {
            label: label.to_owned(),
            choice: MenuChoice::Run {
                command: Command::Run(action),
                window: None,
            },
            mark: None,
            icon: None,
        }
    }

    /// Returns the `vaht` list: vaht's own menu.
    fn vaht_items(&self) -> Vec<MenuItem> {
        let list = |generator: MenuGenerator| {
            Self::submenu_of(generator.label(), self.generate(generator))
        };
        vec![
            list(MenuGenerator::Styles),
            list(MenuGenerator::StyleOptions),
            list(MenuGenerator::Layouts),
            list(MenuGenerator::Positions),
            MenuItem::Separator,
            Self::action_item("Advanced Configuration...", Action::Settings),
            Self::action_item("vaht Command", Action::CommandDialog),
            Self::action_item("Reload Config", Action::Reload),
            Self::action_item("Save Config", Action::SaveSettings),
            MenuItem::Separator,
            Self::action_item("Exit", Action::Exit),
        ]
    }

    /// Returns the `profiles` list: each display profile, the one applied
    /// checked.
    fn profile_items(&self) -> Vec<MenuItem> {
        self.outputs
            .profiles
            .iter()
            .map(|name| MenuItem::Choice {
                label: name.clone(),
                choice: MenuChoice::Run {
                    command: Command::Run(Action::ApplyProfile(name.clone())),
                    window: None,
                },
                mark: Some(self.outputs.profile.as_ref() == Some(name)),
                icon: None,
            })
            .collect()
    }

    /// Returns the `outputs` list: each output on, the primary one
    /// checked.
    fn output_items(&self) -> Vec<MenuItem> {
        self.outputs
            .outputs
            .iter()
            .enumerate()
            .map(|(index, connector)| MenuItem::Choice {
                label: connector.clone(),
                choice: MenuChoice::Run {
                    command: Command::Run(Action::SetPrimaryOutput(connector.clone())),
                    window: None,
                },
                mark: Some(index == 0),
                icon: None,
            })
            .collect()
    }

    /// Returns the placed windows, in the order they opened, a minimized
    /// one's title in parentheses; choosing one brings it forward.
    fn window_list(&self) -> Vec<MenuItem> {
        let manager = self.window_manager;
        let mut windows: Vec<_> = manager
            .windows_top_down()
            .filter(|window| window.is_placed())
            .collect();
        windows.sort_by_key(|window| window.id());
        windows
            .into_iter()
            .map(|window| {
                let title = if window.state().is_minimized {
                    format!("({})", window.title())
                } else {
                    window.title().to_owned()
                };
                MenuItem::Choice {
                    label: title,
                    choice: MenuChoice::Activate(window.id()),
                    mark: None,
                    icon: self.window_icons.get(&window.id()).cloned(),
                }
            })
            .collect()
    }

    /// Returns an item for each of `names`, the one that is `current`
    /// checked, choosing it running `action` on its name.
    fn named_items(
        names: &[String],
        current: Option<&str>,
        action: fn(String) -> Action,
    ) -> Vec<MenuItem> {
        let mut items: Vec<MenuItem> = names
            .iter()
            .map(|name| MenuItem::Choice {
                label: name.clone(),
                choice: MenuChoice::Run {
                    command: Command::Run(action(name.clone())),
                    window: None,
                },
                mark: Some(current == Some(name.as_str())),
                icon: None,
            })
            .collect();
        // The default (vaht's own) comes first, a line between it and the
        // others if there are others.
        if names.first().map(String::as_str) == Some(vaht_ui::Style::DEFAULT_NAME)
            && items.len() > 1
        {
            items.insert(1, MenuItem::Separator);
        }
        items
    }

    /// Returns the items of the list `generator` makes.
    fn generate(&self, generator: MenuGenerator) -> Vec<MenuItem> {
        let manager = self.window_manager;
        match generator {
            MenuGenerator::Workspaces | MenuGenerator::SendToWorkspace => manager
                .workspaces()
                .map(|(workspace, name)| {
                    let target = WorkspaceTarget::Numbered(workspace.number());
                    let (action, window, is_here) = if generator == MenuGenerator::Workspaces {
                        (
                            WindowManagerAction::Workspace(target),
                            None,
                            workspace == manager.current_workspace(),
                        )
                    } else {
                        let is_here = self
                            .window
                            .and_then(|window| manager.window(window))
                            .is_some_and(|window| window.workspace() == workspace);
                        (
                            WindowManagerAction::SendToWorkspace(target),
                            self.window,
                            is_here,
                        )
                    };
                    MenuItem::Choice {
                        label: name.to_owned(),
                        choice: MenuChoice::Run {
                            command: Command::Run(Action::WindowManager(action)),
                            window,
                        },
                        mark: Some(is_here),
                        icon: None,
                    }
                })
                .collect(),
            MenuGenerator::WindowList => self.window_list(),
            MenuGenerator::Styles => {
                Self::named_items(&self.styles, self.current_style, Action::SetStyle)
            }
            MenuGenerator::Layouts => {
                Self::named_items(&self.layouts, self.current_layout, Action::SetLayout)
            }
            MenuGenerator::StyleOptions => Self::style_option_items(&self.style_options),
            MenuGenerator::Positions => Self::named_items(
                &["bottom".to_owned(), "top".to_owned()],
                Some(self.taskbar_edge),
                |edge| {
                    Action::SetSetting(SettingChange::new(
                        &["taskbar", "edge"],
                        &toml::Value::String(edge),
                    ))
                },
            ),
            MenuGenerator::Profiles => self.profile_items(),
            MenuGenerator::Outputs => self.output_items(),
            MenuGenerator::Vaht => self.vaht_items(),
            MenuGenerator::Layer => {
                let current = self.window.and_then(|window| manager.layer_of(window));
                StackLayer::ALL
                    .iter()
                    .map(|(layer, _, label)| MenuItem::Choice {
                        label: (*label).to_owned(),
                        choice: MenuChoice::Run {
                            command: Command::Run(Action::WindowManager(
                                WindowManagerAction::SetLayer(*layer),
                            )),
                            window: self.window,
                        },
                        mark: Some(current == Some(*layer)),
                        icon: None,
                    })
                    .collect()
            }
            MenuGenerator::Alpha => Self::ALPHA_STEPS
                .iter()
                .map(|(label, change)| MenuItem::Choice {
                    label: (*label).to_owned(),
                    choice: MenuChoice::Run {
                        command: Command::Run(Action::WindowManager(
                            WindowManagerAction::SetAlpha(*change),
                        )),
                        window: self.window,
                    },
                    mark: None,
                    icon: None,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use smithay::utils::Point;

    use super::*;

    #[test]
    fn a_tray_menu_becomes_choices_inert_text_and_submenus() {
        let item = TrayItemKey(3);
        let choice = |id, is_enabled| TrayMenuEntry::Choice {
            id,
            label: format!("entry {id}"),
            is_enabled,
            mark: (id == 1).then_some(true),
        };
        let menu = TrayMenu {
            item,
            at: Point::from((0, 0)),
            entries: vec![
                choice(1, true),
                choice(2, false),
                TrayMenuEntry::Separator,
                TrayMenuEntry::Submenu {
                    label: "More".to_owned(),
                    entries: vec![choice(4, true)],
                },
            ],
        };
        let entry = |id, mark| MenuItem::Choice {
            label: format!("entry {id}"),
            choice: MenuChoice::TrayMenuEntry { item, id },
            mark,
            icon: None,
        };
        assert_eq!(
            MenuModel::of_tray_menu(&menu).items,
            [
                entry(1, Some(true)),
                MenuItem::Inert {
                    label: "entry 2".to_owned()
                },
                MenuItem::Separator,
                MenuItem::Submenu {
                    label: "More".to_owned(),
                    menu: MenuModel {
                        title: None,
                        items: vec![entry(4, None)],
                    },
                    icon: None,
                    pipe: None,
                },
            ]
        );
    }

    /// Returns what the items of a list of `names` are: a label for a choice,
    /// none for a line.
    fn labels(names: &[&str]) -> Vec<Option<String>> {
        let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
        MenuBuilder::named_items(&names, None, Action::SetStyle)
            .into_iter()
            .map(|item| match item {
                MenuItem::Choice { label, .. } => Some(label),
                MenuItem::Separator => None,
                MenuItem::Inert { .. } | MenuItem::Submenu { .. } => Some("?".to_owned()),
            })
            .collect()
    }

    #[test]
    fn the_default_comes_first_with_a_line_under_it_only_when_others_follow() {
        let named = |name: &str| Some(name.to_owned());
        assert_eq!(
            labels(&["default", "a", "b"]),
            [named("default"), None, named("a"), named("b")]
        );
        assert_eq!(
            labels(&["default"]),
            [named("default")],
            "nothing to divide it from"
        );
        assert_eq!(
            labels(&["a", "b"]),
            [named("a"), named("b")],
            "no default listed"
        );
    }

    #[test]
    fn the_style_options_list_has_a_submenu_for_each_option_with_the_choice_checked() {
        use crate::config::options::ListedOption;
        let option = ListedOption {
            name: "scheme".to_owned(),
            label: Some("Color scheme".to_owned()),
            checkbox: None,
            checked: None,
            group: None,
            default: "graphite".to_owned(),
            chosen: "blue".to_owned(),
            choices: vec!["graphite".to_owned(), "blue".to_owned()],
            groups: BTreeMap::new(),
        };
        let items = MenuBuilder::style_option_items(&[option]);
        let [
            MenuItem::Submenu { label, menu, .. },
            MenuItem::Separator,
            MenuItem::Choice { label: reset, .. },
        ] = items.as_slice()
        else {
            panic!("one submenu, a line, a reset: {items:?}");
        };
        assert_eq!(reset, "Reset all to defaults");
        assert!(
            matches!(items[2], MenuItem::Choice {
                choice: MenuChoice::Run { command: Command::Run(Action::SetSetting(ref change)), .. },
                ..
            } if change.is_style_option()),
            "the reset changes the options, so the menu stays open"
        );
        assert!(
            matches!(menu.items[1], MenuItem::Separator),
            "a line under the default choice"
        );
        assert_eq!(label, "Color scheme");
        let marks: Vec<(String, Option<bool>)> = menu
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Choice { label, mark, .. } => Some((label.clone(), *mark)),
                MenuItem::Submenu { .. } | MenuItem::Inert { .. } | MenuItem::Separator => None,
            })
            .collect();
        assert_eq!(
            marks,
            [
                ("graphite".to_owned(), Some(false)),
                ("blue".to_owned(), Some(true))
            ]
        );
    }

    #[test]
    fn choices_that_say_their_group_are_listed_in_submenus_after_the_default() {
        use crate::config::options::ListedOption;
        let names = ["graphite", "blue", "teal", "red", "plain"];
        let option = ListedOption {
            name: "palette".to_owned(),
            label: None,
            checkbox: None,
            checked: None,
            group: None,
            default: "graphite".to_owned(),
            chosen: "teal".to_owned(),
            choices: names.iter().map(|name| (*name).to_owned()).collect(),
            groups: [
                ("graphite", "Neutral"),
                ("blue", "Blues"),
                ("teal", "Blues"),
                ("red", "Reds"),
            ]
            .into_iter()
            .map(|(choice, group)| (choice.to_owned(), group.to_owned()))
            .collect(),
        };
        let items = MenuBuilder::style_option_items(&[option]);
        let MenuItem::Submenu { menu, .. } = &items[0] else {
            panic!("the option: {items:?}");
        };
        let labels: Vec<String> = menu
            .items
            .iter()
            .map(|item| match item {
                MenuItem::Choice { label, .. } | MenuItem::Submenu { label, .. } => label.clone(),
                MenuItem::Separator => "-".to_owned(),
                MenuItem::Inert { .. } => "?".to_owned(),
            })
            .collect();
        assert_eq!(
            labels,
            ["graphite", "-", "plain", "Blues", "Reds"],
            "the default, a line, what has no group, then the groups; the default stays first \
             though it names a group"
        );
        let MenuItem::Submenu { menu: blues, .. } = &menu.items[3] else {
            panic!("a group");
        };
        assert!(
            matches!(&blues.items[1], MenuItem::Choice { label, mark: Some(true), .. } if label == "teal"),
            "the chosen choice is checked inside its group: {:?}",
            blues.items
        );
    }

    #[test]
    fn two_choice_options_with_a_label_are_independent_checkboxes_and_the_rest_are_grouped() {
        use crate::config::options::ListedOption;
        let option =
            |name: &str, checkbox: Option<&str>, group: Option<&str>, chosen: &str| ListedOption {
                name: name.to_owned(),
                label: None,
                checkbox: checkbox.map(str::to_owned),
                checked: None,
                group: group.map(str::to_owned),
                default: "a".to_owned(),
                chosen: chosen.to_owned(),
                choices: vec!["a".to_owned(), "b".to_owned()],
                groups: BTreeMap::new(),
            };
        let items = MenuBuilder::style_option_items(&[
            option("faces", Some("Flat faces"), None, "b"),
            option("marks", Some("Classic marks"), None, "a"),
            option("menu", None, None, "a"),
            option("titlebar", None, Some("Sizes"), "a"),
            option("taskbar", None, Some("Sizes"), "a"),
        ]);
        let shape: Vec<String> = items
            .iter()
            .map(|item| match item {
                MenuItem::Choice { label, mark, .. } => format!(
                    "[{}] {label}",
                    match mark {
                        Some(true) => "x",
                        Some(false) => " ",
                        None => "-",
                    }
                ),
                MenuItem::Submenu { label, .. } => format!("{label} >"),
                MenuItem::Separator => "----".to_owned(),
                MenuItem::Inert { label } => label.clone(),
            })
            .collect();
        assert_eq!(
            shape,
            [
                "[x] Flat faces",
                "[ ] Classic marks",
                "----",
                "menu >",
                "Sizes >",
                "----",
                "[-] Reset all to defaults"
            ],
            "each checkbox has its own state; the sizes share a submenu"
        );
        // Checking one asks for its own option's other choice and no other.
        let MenuItem::Choice { choice, .. } = &items[1] else {
            panic!("a checkbox");
        };
        let MenuChoice::Run {
            command: Command::Run(Action::SetSetting(change)),
            ..
        } = choice
        else {
            panic!("a setting");
        };
        assert_eq!(change.to_string(), "style_options.marks \"b\"");
        // Unchecking one asks for the default.
        let MenuItem::Choice { choice, .. } = &items[0] else {
            panic!("a checkbox");
        };
        let MenuChoice::Run {
            command: Command::Run(Action::SetSetting(change)),
            ..
        } = choice
        else {
            panic!("a setting");
        };
        assert_eq!(change.to_string(), "style_options.faces \"a\"");
    }

    #[test]
    fn the_default_styles_options_make_a_short_organized_list() {
        let style = vaht_ui::Style::built_in();
        let listed: Vec<ListedOption> = style
            .options()
            .options
            .iter()
            .map(|option| ListedOption {
                name: option.name.clone(),
                label: option.label.clone(),
                checkbox: option.checkbox.clone(),
                checked: option.checked.clone(),
                group: option.group.clone(),
                default: option.default.clone(),
                chosen: option.chosen.clone(),
                choices: option.choices.clone(),
                groups: option.groups.clone(),
            })
            .collect();
        // The two global checkboxes are loose at the top; the parts are submenus.
        let items = MenuBuilder::style_option_items(&listed);
        let checkboxes_in = |group: &str| -> Vec<(String, Option<bool>)> {
            let menu = items
                .iter()
                .find_map(|item| match item {
                    MenuItem::Submenu { label, menu, .. } if label == group => Some(menu),
                    MenuItem::Submenu { .. }
                    | MenuItem::Choice { .. }
                    | MenuItem::Inert { .. }
                    | MenuItem::Separator => None,
                })
                .expect("the submenu");
            menu.items
                .iter()
                .filter_map(|item| match item {
                    MenuItem::Choice { label, mark, .. } => Some((label.clone(), *mark)),
                    MenuItem::Submenu { .. } | MenuItem::Inert { .. } | MenuItem::Separator => None,
                })
                .collect()
        };
        assert_eq!(
            checkboxes_in("Menus"),
            [("Filled-in menu background".to_owned(), Some(false))]
        );
        let labels: Vec<String> = MenuBuilder::style_option_items(&listed)
            .iter()
            .map(|item| match item {
                MenuItem::Choice { label, .. }
                | MenuItem::Submenu { label, .. }
                | MenuItem::Inert { label } => label.clone(),
                MenuItem::Separator => "-".to_owned(),
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Text Outlines",
                "Softer Gradient Shading",
                "-",
                "Taskbar",
                "Focused windows",
                "Unfocused windows",
                "Menus",
                "Sizes",
                "-",
                "Reset all to defaults"
            ]
        );
    }
}

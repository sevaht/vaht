//! What a menu shows, whoever shows it (01 §6.4).
//!
//! A [`MenuModel`] is a value: a title and items, each with its label, what
//! choosing it does ([`MenuChoice`]) or the submenu it opens, and its mark
//! (a toggle's check). [`MenuBuilder`] makes one from a menu of
//! `menus.toml` and what the window manager knows as the menu opens: toggles
//! read the window's state, and generated lists read the workspaces and
//! windows. The model knows nothing of how it is drawn, so another
//! presenter (a toolkit's menus) can show it alike.

use crate::config::{
    Action, Command, MenuDefinition, MenuEntry, MenuGenerator, Menus, WindowManagerAction,
    WindowToggle,
};
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
    },
    /// An item that opens another menu.
    Submenu {
        /// What it says.
        label: String,
        /// The menu it opens.
        menu: MenuModel,
    },
    /// Text that does nothing.
    Inert {
        /// What it says.
        label: String,
    },
    /// A line between items.
    Separator,
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
}

/// Makes menus from `menus.toml` and the window manager's state.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MenuBuilder<'state> {
    /// The menus as written.
    menus: &'state Menus,
    /// The windows and workspaces.
    window_manager: &'state WindowManager,
    /// The window the menu is about (the window menu's), if any.
    window: Option<WindowId>,
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
    pub(crate) const fn new(
        menus: &'state Menus,
        window_manager: &'state WindowManager,
        window: Option<WindowId>,
    ) -> Self {
        Self {
            menus,
            window_manager,
            window,
        }
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

    /// Returns `definition` as a model, its submenus `depth` deep.
    fn build_definition(&self, definition: &MenuDefinition, depth: usize) -> MenuModel {
        let mut items = Vec::new();
        for entry in &definition.items {
            match entry {
                MenuEntry::Command { label, command } => items.push(MenuItem::Choice {
                    label: label.clone(),
                    choice: MenuChoice::Run {
                        command: command.clone(),
                        window: self.window,
                    },
                    mark: None,
                }),
                MenuEntry::Submenu { label, menu } => {
                    if depth >= Self::MAX_DEPTH {
                        tracing::warn!(menu, "menus nest too deep; not following");
                        continue;
                    }
                    if let Some(submenu) = self.menus.get(menu) {
                        items.push(MenuItem::Submenu {
                            label: label.clone(),
                            menu: self.build_definition(submenu, depth + 1),
                        });
                    }
                }
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
                        items.push(MenuItem::Submenu {
                            label: label
                                .clone()
                                .unwrap_or_else(|| generator.label().to_owned()),
                            menu: MenuModel {
                                title: None,
                                items: generated,
                            },
                        });
                    }
                }
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
        })
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
                    }
                })
                .collect(),
            MenuGenerator::WindowList => {
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
                        }
                    })
                    .collect()
            }
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
                })
                .collect(),
        }
    }
}

//! `settings.toml`: how vaht behaves (02 §3).
//!
//! Every key is optional: a missing one keeps its default, which is what
//! [`Settings::default`] gives and what the built-in `settings.toml` (printed
//! by `vaht check --print-defaults`) says; a unit test keeps the two equal.
//! Sections for features vaht does not have yet are not accepted, so a
//! setting never silently does nothing.

use std::time::Duration;

use crate::placement_strategy::{ColumnDirection, PlacementStrategy, RowDirection};
use crate::window::{TitlebarButton, TitlebarLayout};

/// Every setting.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    /// Which window gets keyboard focus, and when windows rise.
    pub(crate) focus: FocusSettings,
    /// Where new windows go.
    pub(crate) placement: PlacementSettings,
    /// How windows behave and look.
    pub(crate) windows: WindowSettings,
    /// The workspaces.
    pub(crate) workspaces: WorkspaceSettings,
    /// Starting the session.
    pub(crate) session: SessionSettings,
}

/// Which window gets keyboard focus, and when windows rise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FocusSettings {
    /// How focus moves.
    pub(crate) model: FocusModel,
    /// Whether clicking a window raises it.
    pub(crate) raise_on_click: bool,
    /// Whether a window focused by the pointer rises, after
    /// `auto_raise_delay_ms`; with sloppy focus only, as in Fluxbox.
    pub(crate) auto_raise: bool,
    /// How long the pointer must rest on a window before it rises.
    auto_raise_delay_ms: u64,
    /// Whether a new window takes focus.
    pub(crate) focus_new_windows: bool,
}

impl FocusSettings {
    /// Returns how long the pointer must rest on a window before it rises.
    pub(crate) const fn auto_raise_delay(&self) -> Duration {
        Duration::from_millis(self.auto_raise_delay_ms)
    }
}

impl Default for FocusSettings {
    fn default() -> Self {
        Self {
            model: FocusModel::Click,
            raise_on_click: true,
            auto_raise: false,
            auto_raise_delay_ms: 250,
            focus_new_windows: true,
        }
    }
}

/// How keyboard focus moves between windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FocusModel {
    /// A window takes focus when clicked (Fluxbox's `ClickFocus`).
    #[default]
    Click,
    /// A window takes focus when the pointer enters it, and keeps it while
    /// the pointer is on the desktop (Fluxbox's `MouseFocus`).
    Sloppy,
}

/// Where new windows go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct PlacementSettings {
    /// How a place is chosen.
    pub(crate) strategy: PlacementStrategy,
    /// Which way rows are scanned.
    pub(crate) row_direction: RowDirection,
    /// Which way columns are scanned.
    pub(crate) column_direction: ColumnDirection,
}

/// How windows behave and look.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct WindowSettings {
    /// How close, in pixels, a moved window's edge must come to an
    /// output's or another window's edge to snap to it; 0 for never.
    pub(crate) edge_snap_distance: u16,
    /// The longest time between the presses of a double click.
    double_click_ms: u64,
    /// What the frame shows.
    pub(crate) frame: FrameSettings,
}

impl WindowSettings {
    /// Returns the longest time between the presses of a double click.
    pub(crate) const fn double_click_interval(&self) -> Duration {
        Duration::from_millis(self.double_click_ms)
    }
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            edge_snap_distance: 10,
            double_click_ms: 250,
            frame: FrameSettings::default(),
        }
    }
}

/// What a window's frame shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FrameSettings {
    /// The buttons left of the title, left to right.
    pub(crate) titlebar_left: Vec<TitlebarButton>,
    /// The buttons right of the title, left to right.
    pub(crate) titlebar_right: Vec<TitlebarButton>,
}

impl FrameSettings {
    /// Returns the titlebar's button layout.
    pub(crate) fn titlebar_layout(&self) -> TitlebarLayout {
        TitlebarLayout {
            left: self.titlebar_left.clone(),
            right: self.titlebar_right.clone(),
        }
    }
}

impl Default for FrameSettings {
    fn default() -> Self {
        Self {
            titlebar_left: Vec::new(),
            titlebar_right: vec![
                TitlebarButton::Minimize,
                TitlebarButton::Maximize,
                TitlebarButton::Close,
            ],
        }
    }
}

/// The workspaces.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct WorkspaceSettings {
    /// Their names, which also say how many there are; never empty.
    #[serde(deserialize_with = "WorkspaceSettings::deserialize_names")]
    names: Vec<String>,
}

impl WorkspaceSettings {
    /// Returns the workspaces' names; there is at least one.
    pub(crate) fn names(&self) -> &[String] {
        &self.names
    }

    /// Reads the names, refusing an empty list: there is always a workspace.
    fn deserialize_names<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<String>, D::Error> {
        let names: Vec<String> = serde::Deserialize::deserialize(deserializer)?;
        if names.is_empty() {
            return Err(serde::de::Error::custom(
                "there must be at least one workspace",
            ));
        }
        Ok(names)
    }
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            names: (1..=4)
                .map(|number| format!("Workspace {number}"))
                .collect(),
        }
    }
}

/// Starting the session.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SessionSettings {
    /// Command lines run once when vaht starts, as `exec` runs them.
    pub(crate) autostart: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn the_built_in_file_says_what_the_defaults_are() {
        let file = Config::FILES
            .iter()
            .find(|file| file.name == "settings.toml")
            .expect("settings.toml is a configuration file");
        let parsed: Settings = toml::from_str(file.defaults).expect("the built-in file is valid");
        assert_eq!(parsed, Settings::default());
    }

    #[test]
    fn missing_keys_keep_their_defaults() {
        let parsed: Settings =
            toml::from_str("[focus]\nmodel = \"sloppy\"\n").expect("a valid file");
        assert_eq!(parsed.focus.model, FocusModel::Sloppy);
        assert!(parsed.focus.raise_on_click, "the rest stays default");
        assert_eq!(parsed.windows, WindowSettings::default());
    }

    #[test]
    fn there_is_always_a_workspace() {
        let error =
            toml::from_str::<Settings>("[workspaces]\nnames = []\n").expect_err("no workspaces");
        assert!(
            error.message().contains("at least one workspace"),
            "{error}"
        );
    }

    #[test]
    fn settings_for_features_vaht_lacks_are_refused() {
        let error = toml::from_str::<Settings>("[taskbar]\nrows = 2\n").expect_err("no taskbar");
        assert!(error.message().contains("unknown field"), "{error}");
    }
}

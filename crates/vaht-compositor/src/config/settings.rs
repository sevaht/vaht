//! `settings.toml`: how vaht behaves (02 §3).
//!
//! Every key is optional: a missing one keeps its default, which is what
//! [`Settings::default`] gives and what the built-in `settings.toml` (printed
//! by `vaht check --print-defaults`) says; a unit test keeps the two equal.
//! Sections for features vaht does not have yet are not accepted, so a
//! setting never silently does nothing.

mod taskbar;

use std::path::{Path, PathBuf};
use std::time::Duration;

use smithay::utils::{Logical, Point, Size};
use vaht_ui::WallpaperFit;
use vaht_ui::theme::Metrics;

pub(crate) use self::taskbar::{
    TaskbarAlign, TaskbarEdge, TaskbarSettings, TaskbarWidget, WidthPercent,
};
use crate::window_manager::{
    ColumnDirection, PlacementStrategy, Room, RowDirection, TitlebarButton, TitlebarLayout,
    WindowAlpha,
};

/// Every setting.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    /// The theme: `themes/<name>.toml`; the built-in theme when absent.
    pub(crate) theme: Option<String>,
    /// Which window gets keyboard focus, and when windows rise.
    pub(crate) focus: FocusSettings,
    /// Where new windows go.
    pub(crate) placement: PlacementSettings,
    /// How windows behave and look.
    pub(crate) windows: WindowSettings,
    /// The workspaces.
    pub(crate) workspaces: WorkspaceSettings,
    /// The taskbar.
    pub(crate) taskbar: TaskbarSettings,
    /// Menus.
    pub(crate) menus: MenuSettings,
    /// The wallpaper.
    pub(crate) wallpaper: WallpaperSettings,
    /// vaht's IPC socket.
    pub(crate) ipc: IpcSettings,
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

impl PlacementSettings {
    /// Returns where the top-left corner of a new window of `size` (frame
    /// included) goes in `room`.
    pub(crate) fn place(self, size: Size<i32, Logical>, room: &Room) -> Point<i32, Logical> {
        self.strategy
            .place(size, room, self.row_direction, self.column_direction)
    }
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
    /// How opaque a focused window is, 0 to 255.
    focused_alpha: u8,
    /// How opaque an unfocused window is, 0 to 255.
    unfocused_alpha: u8,
    /// Whether a window resized by hand takes each size as the pointer
    /// moves; otherwise an outline shows the size, which the window takes
    /// when the drag ends.
    pub(crate) opaque_resize: bool,
    /// What the frame shows.
    pub(crate) frame: FrameSettings,
}

impl WindowSettings {
    /// Returns the longest time between the presses of a double click.
    pub(crate) const fn double_click_interval(&self) -> Duration {
        Duration::from_millis(self.double_click_ms)
    }

    /// Returns how opaque windows are, focused and not.
    pub(crate) const fn alpha(&self) -> WindowAlpha {
        WindowAlpha {
            focused: self.focused_alpha,
            unfocused: self.unfocused_alpha,
        }
    }
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            edge_snap_distance: 10,
            double_click_ms: 250,
            focused_alpha: 255,
            unfocused_alpha: 255,
            opaque_resize: true,
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
    /// Whether frames have a handle along the bottom, with grips, as tall
    /// as the theme's `window.handle` height.
    pub(crate) handle: bool,
}

impl FrameSettings {
    /// Returns the theme's frame sizes as frames with these settings take
    /// them: without a handle unless they show one.
    pub(crate) const fn frame_metrics(&self, theme: Metrics) -> Metrics {
        if self.handle {
            theme
        } else {
            Metrics {
                handle_height: 0,
                ..theme
            }
        }
    }

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
            handle: false,
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

/// Menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MenuSettings {
    /// How long the pointer rests on a submenu's item before it opens.
    submenu_delay_ms: u64,
    /// How typing in an open menu finds an item.
    pub(crate) search: MenuSearch,
}

/// How typing in an open menu finds an item (Fluxbox's `menuSearch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MenuSearch {
    /// Typing finds nothing.
    Off,
    /// The first item whose label starts with what was typed.
    #[default]
    ItemStart,
    /// The first item whose label has what was typed anywhere in it.
    Anywhere,
}

impl MenuSearch {
    /// Returns whether `label` is found by `typed`, in any letter case.
    pub(crate) fn finds(self, label: &str, typed: &str) -> bool {
        let (label, typed) = (label.to_lowercase(), typed.to_lowercase());
        match self {
            Self::Off => false,
            Self::ItemStart => label.starts_with(&typed),
            Self::Anywhere => label.contains(&typed),
        }
    }
}

impl MenuSettings {
    /// Returns how long the pointer rests on a submenu's item before it
    /// opens.
    pub(crate) const fn submenu_delay(self) -> Duration {
        Duration::from_millis(self.submenu_delay_ms)
    }
}

impl Default for MenuSettings {
    fn default() -> Self {
        Self {
            submenu_delay_ms: 200,
            search: MenuSearch::ItemStart,
        }
    }
}

/// The wallpaper.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct WallpaperSettings {
    /// The image (any common format), as written: `~/` is the home directory, and a
    /// relative path is from the configuration directory. None for no
    /// wallpaper.
    image: Option<String>,
    /// How it fills the output.
    pub(crate) fit: WallpaperFit,
}

impl WallpaperSettings {
    /// Returns the image's path, `~/` read as the home directory and a
    /// relative path taken from `config_directory`; none without an image,
    /// or with `~/` and no home.
    pub(crate) fn image_path(&self, config_directory: &Path) -> Option<PathBuf> {
        let image = self.image.as_deref()?;
        match image.strip_prefix("~/") {
            Some(rest) => Some(PathBuf::from(std::env::var_os("HOME")?).join(rest)),
            None => Some(config_directory.join(image)),
        }
    }
}

/// vaht's IPC socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct IpcSettings {
    /// Whether programs other than vaht's own dialogs may run commands
    /// over IPC; asking for windows and workspaces is always allowed.
    pub(crate) allow_commands: bool,
}

impl Default for IpcSettings {
    fn default() -> Self {
        Self {
            allow_commands: true,
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
    fn a_wallpaper_is_found_from_home_or_the_configuration_directory() {
        let parsed: Settings =
            toml::from_str("[wallpaper]\nimage = \"~/sky.png\"\nfit = \"tile\"\n").expect("valid");
        let home = std::env::var_os("HOME").expect("tests run with a home");
        let config = Path::new("/etc/vaht");
        assert_eq!(
            parsed.wallpaper.image_path(config),
            Some(PathBuf::from(home).join("sky.png"))
        );
        let relative: Settings =
            toml::from_str("[wallpaper]\nimage = \"sky.png\"\n").expect("valid");
        assert_eq!(
            relative.wallpaper.image_path(config),
            Some(PathBuf::from("/etc/vaht/sky.png"))
        );
        assert_eq!(parsed.wallpaper.fit, WallpaperFit::Tile);
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
        let error = toml::from_str::<Settings>("[taskbar]\nrows = 2\n").expect_err("no rows");
        assert!(error.message().contains("unknown field"), "{error}");
    }
}

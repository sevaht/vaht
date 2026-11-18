//! `settings.toml`: how vaht behaves (02 §3).
//!
//! Every key is optional: a missing one keeps its default, which is what
//! [`Settings::default`] gives and what the built-in `settings.toml` (printed
//! by `vaht check --print-defaults`) says; a unit test keeps the two equal.
//! Sections for features vaht does not have yet are not accepted, so a
//! setting never silently does nothing.

mod blur;
mod input;
mod logging;
mod taskbar;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use smithay::utils::{Logical, Point, Size};
use vaht_ui::WallpaperFit;
use vaht_ui::theme::Metrics;

pub(crate) use self::blur::BlurSettings;
pub(crate) use self::input::{
    AccelProfile, AccelSpeed, ClickMethod, DeviceSettings, InputSettings, ScrollMethod, SendEvents,
    TapButtonMap,
};
pub(crate) use self::logging::{FileCount, LoggingSettings};
pub(crate) use self::taskbar::{
    TaskbarAlign, TaskbarEdge, TaskbarSettings, TaskbarWidget, WidthPercent, WindowListScope,
};
use crate::input_emulation::InputEmulationSettings;
use crate::session_environment::ActivationUpdate;
use crate::window_manager::{
    ColumnDirection, PlacementStrategy, Room, RowDirection, TileCell, TitlebarButton,
    TitlebarLayout, WindowAlpha,
};

/// Every setting.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
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
    /// X11 programs.
    pub(crate) xwayland: XwaylandSettings,
    /// How pointing devices behave.
    pub(crate) input: InputSettings,
    /// How big the log file may grow, and how many are kept.
    pub(crate) logging: LoggingSettings,
    /// Whether and how strongly what is behind windows asking for it is
    /// blurred.
    pub(crate) blur: BlurSettings,
}

impl Settings {
    /// Reads the settings from `table`, `settings.toml`'s.
    ///
    /// # Errors
    ///
    /// Returns what is wrong.
    pub(crate) fn from_table(table: toml::Table) -> Result<Self, String> {
        toml::Value::Table(table)
            .try_into()
            .map_err(|error: toml::de::Error| error.message().trim_end().to_owned())
    }
}

/// Which window gets keyboard focus, and when windows rise.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
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
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Default,
    serde::Deserialize,
    serde::Serialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FocusModel {
    /// A window takes focus when clicked (Fluxbox's `ClickFocus`).
    #[default]
    Click,
    /// A window takes focus when the pointer enters it, and keeps it while
    /// the pointer is on the desktop (Fluxbox's `MouseFocus`).
    Sloppy,
    /// As sloppy, but a window takes focus only when the pointer moves
    /// into it from elsewhere: one raised, opened or moved under the
    /// pointer does not, however the pointer moves within it after
    /// (Fluxbox's `StrictMouseFocus`).
    StrictSloppy,
}

impl FocusModel {
    /// Returns whether the pointer moves focus.
    pub(crate) const fn follows_pointer(self) -> bool {
        matches!(self, Self::Sloppy | Self::StrictSloppy)
    }
}

/// Where new windows go.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Deserialize,
    serde::Serialize,
    schemars::JsonSchema,
)]
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
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
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
    /// Snapping a dragged window to the output's edges.
    pub(crate) snapping: SnapSettings,
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
            snapping: SnapSettings::default(),
        }
    }
}

/// Snapping a dragged window to the output's edges (fluxland's snap
/// zones and layout picker).
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SnapSettings {
    /// Whether a window dragged to an edge takes that half, quarter or
    /// the whole usable area when let go.
    pub(crate) enabled: bool,
    /// How close, in pixels, the pointer must come to an edge.
    pub(crate) zone_distance: u16,
    /// Whether dragging to the top edge shows the layout picker.
    pub(crate) layout_picker: bool,
    /// How wide each layout's thumbnail is, in pixels (60 to 300).
    pub(crate) picker_size: u16,
    /// How many thumbnails to a row; 0 for as many as fit.
    pub(crate) picker_columns: u16,
    /// The picker's layouts: each a list of cells, `[x, y, width,
    /// height]` in thousandths of the usable area, as `tile` takes them.
    #[serde(deserialize_with = "SnapSettings::deserialize_layouts")]
    pub(crate) layouts: Vec<Vec<TileCell>>,
}

impl SnapSettings {
    /// Returns the thumbnail width, kept to 60 to 300 pixels.
    pub(crate) fn thumbnail_size(&self) -> u16 {
        self.picker_size.clamp(60, 300)
    }

    /// Reads the layouts, refusing a cell outside the usable area.
    fn deserialize_layouts<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Vec<TileCell>>, D::Error> {
        let layouts: Vec<Vec<[u16; 4]>> = serde::Deserialize::deserialize(deserializer)?;
        layouts
            .into_iter()
            .map(|cells| {
                cells
                    .into_iter()
                    .map(|[x, y, width, height]| {
                        TileCell::new(x, y, width, height).ok_or_else(|| {
                            serde::de::Error::custom(format!(
                                "[{x}, {y}, {width}, {height}] is no cell: thousandths, \
                                 not empty, inside 1000"
                            ))
                        })
                    })
                    .collect()
            })
            .collect()
    }

    /// fluxland's layouts, in two rows of four: splits in two (left heavy,
    /// even, right heavy, top and bottom), then a wide middle, three
    /// columns, a big cell with two beside it, and four quarters.
    fn default_layouts() -> Vec<Vec<TileCell>> {
        let layouts: [&[[u16; 4]]; 8] = [
            &[[0, 0, 667, 1000], [667, 0, 333, 1000]],
            &[[0, 0, 500, 1000], [500, 0, 500, 1000]],
            &[[0, 0, 333, 1000], [333, 0, 667, 1000]],
            &[[0, 0, 1000, 500], [0, 500, 1000, 500]],
            &[[0, 0, 250, 1000], [250, 0, 500, 1000], [750, 0, 250, 1000]],
            &[[0, 0, 333, 1000], [333, 0, 334, 1000], [667, 0, 333, 1000]],
            &[[0, 0, 500, 1000], [500, 0, 500, 500], [500, 500, 500, 500]],
            &[
                [0, 0, 500, 500],
                [500, 0, 500, 500],
                [0, 500, 500, 500],
                [500, 500, 500, 500],
            ],
        ];
        layouts
            .iter()
            .map(|cells| {
                cells
                    .iter()
                    .filter_map(|&[x, y, width, height]| TileCell::new(x, y, width, height))
                    .collect()
            })
            .collect()
    }
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            zone_distance: 10,
            layout_picker: false,
            picker_size: 120,
            picker_columns: 4,
            layouts: Self::default_layouts(),
        }
    }
}

/// What a window's frame shows.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FrameSettings {
    /// The buttons left of the title, left to right.
    pub(crate) titlebar_left: Vec<TitlebarButton>,
    /// The buttons right of the title, left to right.
    pub(crate) titlebar_right: Vec<TitlebarButton>,
    /// Whether frames have a handle along the bottom, with grips, as tall
    /// as the theme's `window.handle` height.
    pub(crate) handle: bool,
    /// Whether the titlebar shows the window's icon left of its title.
    pub(crate) icon: bool,
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
            icon: true,
        }
    }
}

/// The workspaces.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MenuSettings {
    /// How long the pointer rests on a submenu's item before it opens.
    submenu_delay_ms: u64,
    /// How typing in an open menu finds an item.
    pub(crate) search: MenuSearch,
    /// Whether lists of windows show each window's icon.
    pub(crate) window_icons: bool,
}

/// How typing in an open menu finds an item (Fluxbox's `menuSearch`).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Deserialize,
    serde::Serialize,
    schemars::JsonSchema,
)]
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
            window_icons: true,
        }
    }
}

/// The wallpaper.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct WallpaperSettings {
    /// The image (any common format), as written: `~/` is the home directory, and a
    /// relative path is from the configuration directory. None for no
    /// wallpaper.
    image: Option<String>,
    /// How it fills the output.
    pub(crate) fit: WallpaperFit,
    /// Images of particular workspaces, by number from 1, in place of
    /// `image` while they are shown (fluxland's
    /// `session.screen0.workspaceN.wallpaper`).
    #[serde(deserialize_with = "WallpaperSettings::deserialize_workspaces")]
    workspaces: BTreeMap<NonZeroUsize, String>,
}

impl WallpaperSettings {
    /// Returns the image's path, `~/` read as the home directory and a
    /// relative path taken from `config_directory`; none without an image,
    /// or with `~/` and no home.
    pub(crate) fn image_path(&self, config_directory: &Path) -> Option<PathBuf> {
        file_path(self.image.as_deref()?, config_directory)
    }

    /// Returns the path of the image workspace `number` shows in place of
    /// `image`, read as [`Self::image_path`] reads it, and each such
    /// workspace's number.
    pub(crate) fn workspace_image_paths<'settings>(
        &'settings self,
        config_directory: &'settings Path,
    ) -> impl Iterator<Item = (NonZeroUsize, Option<PathBuf>)> + 'settings {
        self.workspaces
            .iter()
            .map(|(number, image)| (*number, file_path(image, config_directory)))
    }

    /// Reads `[wallpaper.workspaces]`, whose keys are workspace numbers
    /// from 1.
    fn deserialize_workspaces<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<NonZeroUsize, String>, D::Error> {
        let written: BTreeMap<String, String> = serde::Deserialize::deserialize(deserializer)?;
        written
            .into_iter()
            .map(|(number, image)| {
                let number = number.parse().map_err(|_not_a_number| {
                    serde::de::Error::custom(format!(
                        "[wallpaper.workspaces] names workspaces by number from 1, not `{number}`"
                    ))
                })?;
                Ok((number, image))
            })
            .collect()
    }
}

/// vaht's IPC socket.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
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

/// X11 programs, through Xwayland.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct XwaylandSettings {
    /// Whether vaht starts Xwayland, so X11 programs run; read at start.
    pub(crate) enabled: bool,
    /// The X resources file loaded into Xwayland when it starts, as xrdb
    /// does in an X session (`~/.Xresources`); none without it.
    pub(crate) resources: Option<String>,
    /// Which X programs may move the pointer and press keys through XTest.
    pub(crate) input_emulation: InputEmulationSettings,
}

impl Default for XwaylandSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            resources: None,
            input_emulation: InputEmulationSettings::default(),
        }
    }
}

impl XwaylandSettings {
    /// Returns the X resources file's path (see [`file_path`]), if one is
    /// set.
    pub(crate) fn resources_path(&self, config_directory: &Path) -> Option<PathBuf> {
        file_path(self.resources.as_deref()?, config_directory)
    }
}

/// Returns the file `path` a setting names: `~/` read as the home
/// directory, and a relative path taken from `config_directory`; none for
/// `~/` with no home.
pub(super) fn file_path(path: &str, config_directory: &Path) -> Option<PathBuf> {
    match path.strip_prefix("~/") {
        Some(rest) => Some(PathBuf::from(std::env::var_os("HOME")?).join(rest)),
        None => Some(config_directory.join(path)),
    }
}

/// Starting the session.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SessionSettings {
    /// Command lines run once when vaht starts, as `exec` runs them.
    pub(crate) autostart: Vec<String>,
    /// When the D-Bus and systemd activation environment is told of the
    /// session, so portals find it; read at start.
    pub(crate) update_activation_environment: ActivationUpdate,
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
        let error = toml::from_str::<Settings>("[taskbar]\ncolumns = 2\n").expect_err("no columns");
        assert!(error.message().contains("unknown field"), "{error}");
    }
}

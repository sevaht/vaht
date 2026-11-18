//! `settings.toml`'s `[taskbar]`: whether there is a taskbar, where, and
//! what it shows (02 §3.1).

use std::fmt;
use std::num::NonZeroU32;
use std::time::Duration;

use jiff::Zoned;
use jiff::civil::date;
use jiff::tz::TimeZone;
use serde::de::{self, Deserializer};

use crate::config::WindowMatcher;

/// The taskbar.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a switch of settings.toml's own, independent of the others"
)]
pub(crate) struct TaskbarSettings {
    /// Whether there is one.
    #[schemars(extend("x-vaht-group" = "Taskbar"))]
    pub(crate) enabled: bool,
    /// How many rows tall it is: a number, or `"auto"` for one row and another
    /// whenever the window list reaches its `row_limit` (and fewer again when
    /// it no longer does), up to `max_rows`.
    #[schemars(extend("x-vaht-group" = "Taskbar"))]
    pub(crate) rows: TaskbarRows,
    /// The most rows an `"auto"` taskbar grows to.
    #[schemars(
        title = "Most rows when automatic",
        extend("x-vaht-group" = "Taskbar")
    )]
    pub(crate) max_rows: NonZeroU32,
    /// What makes an `"auto"` taskbar take another row: entries narrower than
    /// a share of the output's width (`{ percent = 15 }`) or than a number of
    /// pixels (`100`), or more windows in a row than a number
    /// (`{ windows = 8 }`). The taskbar uses the fewest rows that stay within
    /// the limit, up to `max_rows`.
    #[schemars(
        title = "When to take another row",
        extend("x-vaht-group" = "Taskbar")
    )]
    pub(crate) row_limit: RowLimit,
    /// Whether every output has a taskbar, not only `output`.
    #[schemars(extend("x-vaht-group" = "Taskbar"))]
    pub(crate) on_every_output: bool,
    /// The name of the output it shows on, such as `HDMI-A-1`; the
    /// primary output without one, or while that output is not there.
    /// With `on_every_output`, this is the main taskbar, the one with the
    /// tray.
    #[schemars(extend("x-vaht-group" = "Taskbar"))]
    pub(crate) output: Option<String>,
    /// Whose windows a taskbar lists: every output's, or only those on the
    /// taskbar's own output (for a taskbar on every output).
    #[schemars(title = "Windows listed", extend("x-vaht-group" = "Taskbar"))]
    pub(crate) windows: WindowListScope,
    /// The output edge it runs along.
    #[schemars(extend("x-vaht-group" = "Position"))]
    pub(crate) edge: TaskbarEdge,
    /// How much of the edge it spans.
    #[schemars(extend("x-vaht-group" = "Position"))]
    pub(crate) width_percent: WidthPercent,
    /// Where along the edge it sits, when narrower than the edge.
    #[schemars(extend("x-vaht-group" = "Position"))]
    pub(crate) align: TaskbarAlign,
    /// Whether maximized and tiled windows leave it uncovered; an
    /// auto-hiding taskbar never reserves its space.
    #[schemars(extend("x-vaht-group" = "Position"))]
    pub(crate) reserve_space: bool,
    /// Whether it hides until the pointer touches its edge.
    #[schemars(extend("x-vaht-group" = "Hiding"))]
    pub(crate) auto_hide: bool,
    /// How long after the pointer leaves it an auto-hiding taskbar hides.
    #[schemars(extend("x-vaht-group" = "Hiding"))]
    pub(crate) auto_hide_delay_ms: u64,
    /// What it shows, left to right.
    #[schemars(extend("x-vaht-group" = "Contents"))]
    pub(crate) widgets: Vec<TaskbarWidget>,
    /// The window list.
    pub(crate) window_list: WindowListSettings,
    /// Its tooltips.
    pub(crate) tooltips: TooltipSettings,
    /// The clock.
    pub(crate) clock: ClockSettings,
    /// The tray.
    pub(crate) tray: TraySettings,
}

impl Default for TaskbarSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            output: None,
            windows: WindowListScope::default(),
            on_every_output: false,
            edge: TaskbarEdge::Bottom,
            width_percent: WidthPercent::WHOLE,
            rows: TaskbarRows::default(),
            max_rows: NonZeroU32::new(3).expect("3 is not 0"),
            row_limit: RowLimit::default(),
            align: TaskbarAlign::Center,
            reserve_space: true,
            auto_hide: false,
            auto_hide_delay_ms: 500,
            tooltips: TooltipSettings::default(),
            widgets: vec![
                TaskbarWidget::WorkspacePrevious,
                TaskbarWidget::WorkspaceName,
                TaskbarWidget::WorkspaceNext,
                TaskbarWidget::WindowList,
                TaskbarWidget::Tray,
                TaskbarWidget::Clock,
            ],
            window_list: WindowListSettings::default(),
            clock: ClockSettings::default(),
            tray: TraySettings::default(),
        }
    }
}

impl TaskbarSettings {
    /// Returns the fewest and the most rows the taskbar is.
    pub(crate) const fn row_range(&self) -> (u32, u32) {
        match self.rows {
            TaskbarRows::Fixed(rows) => (rows.get(), rows.get()),
            TaskbarRows::Auto => (1, self.max_rows.get()),
        }
    }

    /// Returns how long the pointer rests before a tooltip shows; none
    /// when tooltips are off.
    pub(crate) const fn tooltip_delay(&self) -> Option<Duration> {
        if self.tooltips.enabled {
            Some(Duration::from_millis(self.tooltips.delay_ms))
        } else {
            None
        }
    }

    /// Returns how long after the pointer leaves it an auto-hiding taskbar
    /// hides; none when it does not hide.
    pub(crate) const fn hide_delay(&self) -> Option<Duration> {
        if self.auto_hide {
            Some(Duration::from_millis(self.auto_hide_delay_ms))
        } else {
            None
        }
    }
}

/// How many rows the taskbar is: always a number of them (`rows = 2`), or
/// (`"auto"`) one, and another whenever the window list's entries would be
/// narrower than its `row_limit` allows (or hold more windows), up to `max_rows`. Each row is as
/// tall as the style's taskbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TaskbarRows {
    /// Always this many.
    Fixed(NonZeroU32),
    /// As many as the window list needs.
    Auto,
}

impl Default for TaskbarRows {
    fn default() -> Self {
        Self::Fixed(NonZeroU32::MIN)
    }
}

impl serde::Serialize for TaskbarRows {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Fixed(rows) => serializer.serialize_u32(rows.get()),
            Self::Auto => serializer.serialize_str("auto"),
        }
    }
}

impl schemars::JsonSchema for TaskbarRows {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TaskbarRows".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "oneOf": [
                {
                    "type": "integer", "minimum": 1,
                    "title": "A fixed number",
                    "description": "Always this many rows.",
                },
                {
                    "const": "auto", "title": "Automatic",
                    "description": "One row, and another whenever the window list's entries would be narrower than its minimum entry width, up to the most rows.",
                },
            ],
        })
    }
}

impl<'de> serde::Deserialize<'de> for TaskbarRows {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// What `rows` may say.
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Given {
            /// A number of rows.
            Count(i64),
            /// A keyword: `"auto"`.
            Keyword(String),
        }
        match Given::deserialize(deserializer)? {
            Given::Count(count) => u32::try_from(count)
                .ok()
                .and_then(NonZeroU32::new)
                .map(Self::Fixed)
                .ok_or_else(|| {
                    de::Error::custom(format!("{count} is no number of rows: at least 1"))
                }),
            Given::Keyword(keyword) if keyword == "auto" => Ok(Self::Auto),
            Given::Keyword(other) => Err(de::Error::custom(format!(
                "`rows` is a number of rows or \"auto\", not `{other}`"
            ))),
        }
    }
}

/// How much of its edge the taskbar spans, as a whole percentage from 1 to
/// 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WidthPercent(u32);

impl WidthPercent {
    /// The whole edge.
    pub(crate) const WHOLE: Self = Self(100);

    /// Returns the percentage, or `None` outside 1 to 100.
    pub(crate) const fn new(percent: u32) -> Option<Self> {
        if percent >= 1 && percent <= Self::WHOLE.0 {
            Some(Self(percent))
        } else {
            None
        }
    }

    /// Returns this share of `length`, at least 1.
    pub(crate) fn of(self, length: i32) -> i32 {
        let share = i64::from(length) * i64::from(self.0) / i64::from(Self::WHOLE.0);
        i32::try_from(share).unwrap_or(length).max(1)
    }
}

impl serde::Serialize for WidthPercent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}

impl schemars::JsonSchema for WidthPercent {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WidthPercent".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        crate::config::schema::bounded_integer(1, i64::from(Self::WHOLE.0))
    }
}

impl<'de> serde::Deserialize<'de> for WidthPercent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let percent = u32::deserialize(deserializer)?;
        Self::new(percent).ok_or_else(|| {
            de::Error::custom(format!("{percent} is no width: it is 1 to 100 percent"))
        })
    }
}

/// The tray: which items go first and last.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct TraySettings {
    /// Items first, in this order, named by their id in any case.
    pub(crate) pin_left: Vec<String>,
    /// Items last, in this order.
    pub(crate) pin_right: Vec<String>,
    /// How long after a click on an item its program may bring its
    /// windows forward without a token from the focused program, as the
    /// item's own menu often cannot pass one; 0 for never.
    pub(crate) activation_grace_ms: u64,
}

impl Default for TraySettings {
    fn default() -> Self {
        Self {
            pin_left: Vec::new(),
            pin_right: Vec::new(),
            activation_grace_ms: 500,
        }
    }
}

impl TraySettings {
    /// Returns how long after a click on an item its program may bring its
    /// windows forward.
    pub(crate) const fn activation_grace(&self) -> Duration {
        Duration::from_millis(self.activation_grace_ms)
    }
}

/// Where along its edge a taskbar narrower than the edge sits.
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
pub(crate) enum TaskbarAlign {
    /// At the left end.
    Left,
    /// In the middle.
    #[default]
    Center,
    /// At the right end.
    Right,
}

impl TaskbarAlign {
    /// Returns where a taskbar `width` wide starts along an edge from
    /// `start`, `length` long.
    pub(crate) const fn start(self, start: i32, length: i32, width: i32) -> i32 {
        match self {
            Self::Left => start,
            Self::Center => start + (length - width) / 2,
            Self::Right => start + length - width,
        }
    }
}

/// The output edge the taskbar runs along.
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
pub(crate) enum TaskbarEdge {
    /// The top.
    Top,
    /// The bottom.
    #[default]
    Bottom,
}

/// Something the taskbar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TaskbarWidget {
    /// A button showing the workspace before.
    WorkspacePrevious,
    /// The name of the workspace shown.
    WorkspaceName,
    /// A button showing the workspace after.
    WorkspaceNext,
    /// A button focusing the window before in the window list (Fluxbox's
    /// `prevwindow`).
    WindowPrevious,
    /// A button focusing the window after in the window list (Fluxbox's
    /// `nextwindow`).
    WindowNext,
    /// An entry per window.
    WindowList,
    /// The time.
    Clock,
    /// Programs' status icons.
    Tray,
}

impl TaskbarWidget {
    /// The widgets by name.
    const NAMES: [(&str, Self); 8] = [
        ("workspace-previous", Self::WorkspacePrevious),
        ("workspace-name", Self::WorkspaceName),
        ("workspace-next", Self::WorkspaceNext),
        ("window-previous", Self::WindowPrevious),
        ("window-next", Self::WindowNext),
        ("window-list", Self::WindowList),
        ("tray", Self::Tray),
        ("clock", Self::Clock),
    ];
}

impl serde::Serialize for TaskbarWidget {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let name = Self::NAMES
            .iter()
            .find(|(_, widget)| widget == self)
            .map(|(name, _)| *name)
            .expect("every widget is in NAMES");
        serializer.serialize_str(name)
    }
}

impl schemars::JsonSchema for TaskbarWidget {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TaskbarWidget".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let names: Vec<&str> = Self::NAMES.iter().map(|(name, _)| *name).collect();
        schemars::json_schema!({ "enum": names })
    }
}

impl<'de> serde::Deserialize<'de> for TaskbarWidget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::NAMES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|&(_, widget)| widget)
            .ok_or_else(|| {
                de::Error::custom(format!(
                    "`{name}` is no taskbar widget; they are workspace-previous, \
                     workspace-name, workspace-next, window-previous, window-next, \
                     window-list, tray and clock"
                ))
            })
    }
}

/// The taskbar's window list.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct WindowListSettings {
    /// Which windows have an entry.
    pub(crate) show: WindowMatcher,
    /// How wide an entry may be.
    pub(crate) entry_width: EntryWidth,
    /// Whether entries show their application's icon.
    pub(crate) icons: bool,
}

impl Default for WindowListSettings {
    fn default() -> Self {
        Self {
            show: WindowMatcher::on_current_workspace(),
            entry_width: EntryWidth::default(),
            icons: true,
        }
    }
}

/// Whose windows a taskbar's window list lists.
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
pub(crate) enum WindowListScope {
    /// Every output's.
    #[default]
    All,
    /// Those mostly on the taskbar's own output.
    OwnOutput,
}

/// How wide a window list's entries are: equal shares of the whole list
/// (`"fill"`, Fluxbox's `Relative` alignment), or of it up to a width
/// (`{ max = <pixels> }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum EntryWidth {
    /// Equal shares of the whole list.
    #[default]
    Fill,
    /// Equal shares, each at most this many pixels.
    AtMost(u32),
}

impl EntryWidth {
    /// Returns the most an entry is wide, in pixels: no limit when the
    /// entries fill the list.
    pub(crate) const fn max(self) -> u32 {
        match self {
            Self::Fill => u32::MAX,
            Self::AtMost(max) => max,
        }
    }
}

impl serde::Serialize for EntryWidth {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        match self {
            Self::Fill => serializer.serialize_str("fill"),
            Self::AtMost(max) => {
                let mut limit = serializer.serialize_map(Some(1))?;
                limit.serialize_entry("max", max)?;
                limit.end()
            }
        }
    }
}

impl schemars::JsonSchema for EntryWidth {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "EntryWidth".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "oneOf": [
                {
                    "const": "fill", "title": "Fill the list",
                    "description": "Equal shares of the whole list.",
                },
                {
                    "type": "object", "title": "At most",
                    "description": "Equal shares of the list, each at most this many pixels wide.",
                    "properties": { "max": { "type": "integer", "minimum": 1, "title": "Pixels" } },
                    "required": ["max"],
                    "additionalProperties": false,
                },
            ],
        })
    }
}

impl<'de> serde::Deserialize<'de> for EntryWidth {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// What `entry_width` may say.
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Given {
            /// A keyword: `"fill"`.
            Keyword(String),
            /// A limit.
            Limit(Limit),
        }
        /// `{ max = <pixels> }`.
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Limit {
            /// The most an entry is wide, in pixels.
            max: u32,
        }
        match Given::deserialize(deserializer) {
            Ok(Given::Keyword(keyword)) if keyword == "fill" => Ok(Self::Fill),
            Ok(Given::Limit(Limit { max })) => Ok(Self::AtMost(max)),
            Ok(Given::Keyword(_)) | Err(_) => Err(de::Error::custom(
                "entry_width is \"fill\" or { max = <pixels> }",
            )),
        }
    }
}

/// What makes an `"auto"` taskbar take another row: its window list's entries
/// getting too narrow, as a number of pixels (`row_limit = 100`) or a share of
/// the width of the output the taskbar is on (`{ percent = 15 }`, so one
/// setting suits a laptop's screen and a television), or a row holding too
/// many windows (`{ windows = 8 }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowLimit {
    /// Entries narrower than this many pixels.
    Pixels(u32),
    /// Entries narrower than this share of the output's width.
    Percent(WidthPercent),
    /// More windows than this in a row.
    Windows(NonZeroU32),
}

impl RowLimit {
    /// The share of the output's width that is the default.
    const DEFAULT_PERCENT: u32 = 15;

    /// The number of windows a row holds, when the limit is a count, that
    /// the schema suggests.
    const SUGGESTED_WINDOWS: u32 = 8;

    /// Returns the narrowest an entry may be, in pixels, on an output
    /// `output_width` wide; none when the limit is a number of windows.
    pub(crate) fn narrowest_entry(self, output_width: i32) -> Option<u32> {
        match self {
            Self::Pixels(pixels) => Some(pixels),
            Self::Percent(percent) => Some(u32::try_from(percent.of(output_width)).unwrap_or(0)),
            Self::Windows(_) => None,
        }
    }

    /// Returns how many rows `windows` windows need, when the limit is a
    /// number of windows in a row; none when it is a width.
    pub(crate) fn rows_for(self, windows: usize) -> Option<u32> {
        match self {
            Self::Windows(most) => {
                let per_row = usize::try_from(most.get()).unwrap_or(usize::MAX);
                Some(
                    u32::try_from(windows.div_ceil(per_row))
                        .unwrap_or(u32::MAX)
                        .max(1),
                )
            }
            Self::Pixels(_) | Self::Percent(_) => None,
        }
    }
}

impl Default for RowLimit {
    fn default() -> Self {
        Self::Percent(
            WidthPercent::new(Self::DEFAULT_PERCENT).expect("the default share is from 1 to 100"),
        )
    }
}

impl serde::Serialize for RowLimit {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        match self {
            Self::Pixels(pixels) => serializer.serialize_u32(*pixels),
            Self::Percent(percent) => {
                let mut share = serializer.serialize_map(Some(1))?;
                share.serialize_entry("percent", percent)?;
                share.end()
            }
            Self::Windows(windows) => {
                let mut count = serializer.serialize_map(Some(1))?;
                count.serialize_entry("windows", windows)?;
                count.end()
            }
        }
    }
}

impl schemars::JsonSchema for RowLimit {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RowLimit".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "oneOf": [
                {
                    "type": "object",
                    "title": "Entry narrower than a share of the output's width",
                    "description": "Another row when an entry would be narrower than this share of the width of the output the taskbar is on, so the setting suits a small screen and a large one alike.",
                    "properties": {
                        "percent": {
                            "type": "integer", "title": "Percent",
                            "minimum": 1, "maximum": WidthPercent::WHOLE.0,
                        },
                    },
                    "required": ["percent"],
                    "additionalProperties": false,
                    "default": { "percent": Self::DEFAULT_PERCENT },
                },
                {
                    "type": "integer", "minimum": 0,
                    "title": "Entry narrower than a number of pixels",
                    "description": "Another row when an entry would be narrower than this many pixels, whatever the output.",
                    "default": 200,
                },
                {
                    "type": "object",
                    "title": "More windows in a row than a number",
                    "description": "Another row when a row would hold more than this many windows.",
                    "properties": {
                        "windows": {
                            "type": "integer", "title": "Windows", "minimum": 1,
                        },
                    },
                    "required": ["windows"],
                    "additionalProperties": false,
                    "default": { "windows": Self::SUGGESTED_WINDOWS },
                },
            ],
        })
    }
}

impl<'de> serde::Deserialize<'de> for RowLimit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// What `row_limit` may say.
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Given {
            /// A number of pixels.
            Pixels(u32),
            /// A share of the output's width.
            Share(Share),
            /// A number of windows in a row.
            Count(Count),
        }
        /// `{ percent = <1 to 100> }`.
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Share {
            /// The share, as a whole percentage.
            percent: u32,
        }
        /// `{ windows = <1 or more> }`.
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Count {
            /// The most windows in a row.
            windows: u32,
        }
        let message = "row_limit is a number of pixels, { percent = <1 to 100> } \
                       or { windows = <1 or more> }";
        match Given::deserialize(deserializer) {
            Ok(Given::Pixels(pixels)) => Ok(Self::Pixels(pixels)),
            Ok(Given::Share(Share { percent })) => WidthPercent::new(percent)
                .map(Self::Percent)
                .ok_or_else(|| de::Error::custom(message)),
            Ok(Given::Count(Count { windows })) => NonZeroU32::new(windows)
                .map(Self::Windows)
                .ok_or_else(|| de::Error::custom(message)),
            Err(_) => Err(de::Error::custom(message)),
        }
    }
}

/// The taskbar's clock.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ClockSettings {
    /// What it shows.
    pub(crate) format: ClockFormat,
}

/// A clock's format, as `strftime` writes it: checked when read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClockFormat(String);

impl ClockFormat {
    /// Returns `time` in this format.
    pub(crate) fn format(&self, time: &Zoned) -> String {
        jiff::fmt::strtime::format(&self.0, time).unwrap_or_else(|error| format!("{error}"))
    }

    /// Returns whether the clock shows seconds: whether two times a second
    /// apart read differently.
    pub(crate) fn shows_seconds(&self) -> bool {
        let at = |second| {
            date(2026, 1, 1)
                .at(12, 0, second, 0)
                .to_zoned(TimeZone::UTC)
                .map(|time| self.format(&time))
                .ok()
        };
        at(0) != at(1)
    }
}

impl Default for ClockFormat {
    fn default() -> Self {
        Self("%k:%M".to_owned())
    }
}

impl fmt::Display for ClockFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl serde::Serialize for ClockFormat {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl schemars::JsonSchema for ClockFormat {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ClockFormat".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // Checked as it is read, against `strftime`'s conversions.
        schemars::json_schema!({ "type": "string", "description": "A strftime format, such as %k:%M." })
    }
}

impl<'de> serde::Deserialize<'de> for ClockFormat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let format = String::deserialize(deserializer)?;
        let sample = date(2026, 1, 1)
            .at(12, 0, 0, 0)
            .to_zoned(TimeZone::UTC)
            .map_err(de::Error::custom)?;
        jiff::fmt::strtime::format(&format, &sample).map_err(|error| {
            de::Error::custom(format!(
                "the clock's format `{format}` is not valid: {error}"
            ))
        })?;
        Ok(Self(format))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Parses a `[taskbar]` table.
    fn taskbar(text: &str) -> Result<TaskbarSettings, String> {
        toml::from_str(text).map_err(|error| error.message().to_owned())
    }

    #[rstest]
    #[case::fill("\"fill\"", EntryWidth::Fill)]
    #[case::limit("{ max = 200 }", EntryWidth::AtMost(200))]
    fn an_entry_width_fills_the_list_or_stops_at_a_width(
        #[case] given: &str,
        #[case] expected: EntryWidth,
    ) {
        let parsed = taskbar(&format!("[window_list]\nentry_width = {given}\n")).expect("valid");
        assert_eq!(parsed.window_list.entry_width, expected);
    }

    #[test]
    fn entries_fill_the_list_unless_told_otherwise() {
        let parsed = taskbar("").expect("valid");
        assert_eq!(parsed.window_list.entry_width.max(), u32::MAX);
    }

    #[rstest]
    #[case::other_keyword("\"relative\"")]
    #[case::other_key("{ min = 20 }")]
    fn an_entry_width_neither_fill_nor_a_limit_is_refused(#[case] given: &str) {
        let error =
            taskbar(&format!("[window_list]\nentry_width = {given}\n")).expect_err("refused");
        assert!(error.contains("\"fill\" or { max = <pixels> }"), "{error}");
    }

    #[rstest]
    #[case::minutes("%k:%M", false)]
    #[case::seconds("%H:%M:%S", true)]
    #[case::day("%a %d %b", false)]
    fn a_clock_knows_whether_it_shows_seconds(#[case] format: &str, #[case] seconds: bool) {
        let parsed = taskbar(&format!("[clock]\nformat = \"{format}\"\n")).expect("valid");
        assert_eq!(parsed.clock.format.shows_seconds(), seconds);
    }

    #[test]
    fn a_clock_writes_the_time_as_its_format_says() {
        let format = ClockFormat("%k:%M".to_owned());
        let time = date(2026, 3, 4)
            .at(7, 5, 0, 0)
            .to_zoned(TimeZone::UTC)
            .expect("a valid time");
        assert_eq!(format.format(&time), " 7:05");
    }

    #[rstest]
    #[case::one_by_default("", (1, 1))]
    #[case::fixed("rows = 2\n", (2, 2))]
    #[case::auto("rows = \"auto\"\n", (1, 3))]
    #[case::auto_to_five("rows = \"auto\"\nmax_rows = 5\n", (1, 5))]
    fn rows_are_fixed_or_grow_up_to_a_limit(#[case] text: &str, #[case] range: (u32, u32)) {
        assert_eq!(taskbar(text).expect("valid").row_range(), range);
    }

    #[rstest]
    #[case::a_share_by_default("", Some(288))]
    #[case::pixels("row_limit = 100\n", Some(100))]
    #[case::a_share("row_limit = { percent = 25 }\n", Some(480))]
    #[case::the_whole_width("row_limit = { percent = 100 }\n", Some(1920))]
    #[case::a_count("row_limit = { windows = 8 }\n", None)]
    fn the_narrowest_entry_is_pixels_or_a_share_of_the_output(
        #[case] text: &str,
        #[case] pixels: Option<u32>,
    ) {
        let limit = taskbar(text).expect("valid").row_limit;
        assert_eq!(limit.narrowest_entry(1920), pixels);
    }

    #[test]
    fn the_narrowest_entry_follows_the_width_of_the_output() {
        let limit = RowLimit::default();
        assert_eq!(
            limit.narrowest_entry(1920),
            Some(288),
            "15% of a 1080p output"
        );
        assert_eq!(
            limit.narrowest_entry(1280),
            Some(192),
            "15% of a smaller one"
        );
        assert_eq!(RowLimit::Pixels(100).narrowest_entry(1280), Some(100));
    }

    #[rstest]
    #[case::none(0, 1)]
    #[case::one(1, 1)]
    #[case::a_full_row(8, 1)]
    #[case::one_over(9, 2)]
    #[case::two_full_rows(16, 2)]
    #[case::three(17, 3)]
    fn a_count_of_windows_takes_a_row_for_each_set_of_that_many(
        #[case] windows: usize,
        #[case] rows: u32,
    ) {
        let limit = RowLimit::Windows(NonZeroU32::new(8).expect("8 is not 0"));
        assert_eq!(limit.rows_for(windows), Some(rows));
        assert_eq!(
            RowLimit::default().rows_for(windows),
            None,
            "a width is laid out"
        );
    }

    #[rstest]
    #[case::no_share("row_limit = { percent = 0 }\n")]
    #[case::too_much("row_limit = { percent = 101 }\n")]
    #[case::no_windows("row_limit = { windows = 0 }\n")]
    #[case::a_word("row_limit = \"wide\"\n")]
    #[case::another_key("row_limit = { max = 5 }\n")]
    fn a_limit_that_is_none_of_the_three_is_refused(#[case] text: &str) {
        let error = taskbar(text).expect_err(text);
        assert!(error.contains("row_limit is a number of pixels"), "{error}");
    }

    #[rstest]
    #[case::unknown_widget("widgets = [\"pager\"]\n", "no taskbar widget")]
    #[case::bad_format("[clock]\nformat = \"%k:%\"\n", "not valid")]
    #[case::no_rows("rows = 0\n", "at least 1")]
    #[case::bad_rows("rows = \"many\"\n", "not `many`")]
    #[case::no_max_rows("rows = \"auto\"\nmax_rows = 0\n", "nonzero")]
    fn mistakes_are_explained(#[case] text: &str, #[case] expected: &str) {
        let error = taskbar(text).expect_err(text);
        assert!(error.contains(expected), "{error}");
    }
}

/// The taskbar's tooltips.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct TooltipSettings {
    /// Whether resting the pointer on an entry whose title does not fit,
    /// or on a tray item, shows a tooltip.
    pub(crate) enabled: bool,
    /// How long the pointer rests before a tooltip shows.
    pub(crate) delay_ms: u64,
}

impl Default for TooltipSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            delay_ms: 500,
        }
    }
}

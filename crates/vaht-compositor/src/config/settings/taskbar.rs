//! `settings.toml`'s `[taskbar]`: whether there is a taskbar, where, and
//! what it shows (02 §3.1).

use std::fmt;
use std::time::Duration;

use jiff::Zoned;
use jiff::civil::date;
use jiff::tz::TimeZone;
use serde::de::{self, Deserializer};

use crate::config::WindowMatcher;

/// The taskbar.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct TaskbarSettings {
    /// Whether there is one.
    pub(crate) enabled: bool,
    /// The name of the output it shows on, such as `HDMI-A-1`; the
    /// primary output without one, or while that output is not there.
    pub(crate) output: Option<String>,
    /// The output edge it runs along.
    pub(crate) edge: TaskbarEdge,
    /// How much of the edge it spans.
    pub(crate) width_percent: WidthPercent,
    /// Where along the edge it sits, when narrower than the edge.
    pub(crate) align: TaskbarAlign,
    /// Whether maximized and tiled windows leave it uncovered; an
    /// auto-hiding taskbar never reserves its space.
    pub(crate) reserve_space: bool,
    /// Whether it hides until the pointer touches its edge.
    pub(crate) auto_hide: bool,
    /// How long after the pointer leaves it an auto-hiding taskbar hides.
    pub(crate) auto_hide_delay_ms: u64,
    /// Its tooltips.
    pub(crate) tooltips: TooltipSettings,
    /// What it shows, left to right.
    pub(crate) widgets: Vec<TaskbarWidget>,
    /// The window list.
    pub(crate) window_list: WindowListSettings,
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
            edge: TaskbarEdge::Bottom,
            width_percent: WidthPercent::WHOLE,
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

impl<'de> serde::Deserialize<'de> for WidthPercent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let percent = u32::deserialize(deserializer)?;
        Self::new(percent).ok_or_else(|| {
            de::Error::custom(format!("{percent} is no width: it is 1 to 100 percent"))
        })
    }
}

/// The tray: which items go first and last.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Deserialize)]
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
    /// An entry per window.
    WindowList,
    /// The time.
    Clock,
    /// Programs' status icons.
    Tray,
}

impl TaskbarWidget {
    /// The widgets by name.
    const NAMES: [(&str, Self); 6] = [
        ("workspace-previous", Self::WorkspacePrevious),
        ("workspace-name", Self::WorkspaceName),
        ("workspace-next", Self::WorkspaceNext),
        ("window-list", Self::WindowList),
        ("tray", Self::Tray),
        ("clock", Self::Clock),
    ];

    /// Widgets of 02 §3.1 that vaht does not have yet.
    const PLANNED: [&str; 2] = ["window-previous", "window-next"];
}

impl<'de> serde::Deserialize<'de> for TaskbarWidget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        if Self::PLANNED.contains(&name.as_str()) {
            return Err(de::Error::custom(format!(
                "the `{name}` widget is not available in vaht yet"
            )));
        }
        Self::NAMES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|&(_, widget)| widget)
            .ok_or_else(|| {
                de::Error::custom(format!(
                    "`{name}` is no taskbar widget; they are workspace-previous, \
                     workspace-name, workspace-next, window-list, tray and clock"
                ))
            })
    }
}

/// The taskbar's window list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
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

/// How wide a window list's entries are: equal shares of the list, up to
/// `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EntryWidth {
    /// The most an entry is wide, in pixels.
    pub(crate) max: u32,
}

impl Default for EntryWidth {
    fn default() -> Self {
        Self { max: 128 }
    }
}

/// The taskbar's clock.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
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
    #[case::planned_widget("widgets = [\"window-previous\"]\n", "not available in vaht yet")]
    #[case::unknown_widget("widgets = [\"pager\"]\n", "no taskbar widget")]
    #[case::bad_format("[clock]\nformat = \"%k:%\"\n", "not valid")]
    #[case::planned_key("rows = 2\n", "unknown field")]
    fn mistakes_are_explained(#[case] text: &str, #[case] expected: &str) {
        let error = taskbar(text).expect_err(text);
        assert!(error.contains(expected), "{error}");
    }
}

/// The taskbar's tooltips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
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

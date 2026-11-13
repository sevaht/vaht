//! `settings.toml`'s `[taskbar]`: whether there is a taskbar, where, and
//! what it shows (02 §3.1).

use std::fmt;

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
    /// The output edge it runs along.
    pub(crate) edge: TaskbarEdge,
    /// Whether maximized and tiled windows leave it uncovered.
    pub(crate) reserve_space: bool,
    /// What it shows, left to right.
    pub(crate) widgets: Vec<TaskbarWidget>,
    /// The window list.
    pub(crate) window_list: WindowListSettings,
    /// The clock.
    pub(crate) clock: ClockSettings,
}

impl Default for TaskbarSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            edge: TaskbarEdge::Bottom,
            reserve_space: true,
            widgets: vec![
                TaskbarWidget::WorkspacePrevious,
                TaskbarWidget::WorkspaceName,
                TaskbarWidget::WorkspaceNext,
                TaskbarWidget::WindowList,
                TaskbarWidget::Clock,
            ],
            window_list: WindowListSettings::default(),
            clock: ClockSettings::default(),
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
}

impl TaskbarWidget {
    /// The widgets by name.
    const NAMES: [(&str, Self); 5] = [
        ("workspace-previous", Self::WorkspacePrevious),
        ("workspace-name", Self::WorkspaceName),
        ("workspace-next", Self::WorkspaceNext),
        ("window-list", Self::WindowList),
        ("clock", Self::Clock),
    ];

    /// Widgets of 02 §3.1 that vaht does not have yet.
    const PLANNED: [&str; 3] = ["tray", "window-previous", "window-next"];
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
                     workspace-name, workspace-next, window-list and clock"
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
    #[case::planned_widget("widgets = [\"tray\"]\n", "not available in vaht yet")]
    #[case::unknown_widget("widgets = [\"pager\"]\n", "no taskbar widget")]
    #[case::bad_format("[clock]\nformat = \"%k:%\"\n", "not valid")]
    #[case::planned_key("rows = 2\n", "unknown field")]
    fn mistakes_are_explained(#[case] text: &str, #[case] expected: &str) {
        let error = taskbar(text).expect_err(text);
        assert!(error.contains(expected), "{error}");
    }
}

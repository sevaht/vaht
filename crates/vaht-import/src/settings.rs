//! Converting Fluxbox's `init` to `settings.toml` (02 §3).
//!
//! Each resource vaht has a setting for becomes that setting, written only
//! when it differs from vaht's default: the built-in `settings.toml` is read
//! for the defaults, so they are written down in one place. Every other
//! resource is reported: dropped (tabs, the slit, X11 tuning) or not in vaht
//! yet.

use toml_edit::{Array, DocumentMut, Item, Table, Value};
use vaht_compositor::Config;

use crate::pattern::ClientPattern;
use crate::report::{Reason, Report};
use crate::resources::Resources;

/// The prefix of the resources of Fluxbox's first (and, on Wayland, only)
/// screen.
const SCREEN: &str = "session.screen0.";

/// Resource name fragments vaht deliberately drops, and why.
const DROPPED: [(&str, &str); 14] = [
    ("tabs.", "tabs are gone"),
    ("tab.", "tabs are gone"),
    ("tabFocusModel", "tabs are gone"),
    ("tabsAttachArea", "tabs are gone"),
    ("tabPadding", "tabs are gone"),
    ("slit", "the slit is gone"),
    ("colorsPerChannel", "no effect on Wayland"),
    ("cacheMax", "no effect on Wayland"),
    ("cacheLife", "no effect on Wayland"),
    ("configVersion", "no effect on Wayland"),
    ("forcePseudoTransparency", "no effect on Wayland"),
    ("ignoreBorder", "the border always moves the window"),
    ("File", "vaht's file names are fixed"),
    (
        "styleOverlay",
        "themes cannot set the wallpaper, so overlays are not needed",
    ),
];

/// The parts of a frame Fluxbox's `defaultDeco` keeps, which vaht can
/// draw: a preset name or a mask of `DECORM_*` bits
/// (`WindowState::getDecoMaskFromString`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Decoration {
    /// Whether frames have a titlebar.
    has_titlebar: bool,
    /// Whether frames have a handle.
    has_handle: bool,
}

impl Decoration {
    /// Fluxbox's `DECORM_TITLEBAR` bit.
    const TITLEBAR_BIT: i64 = 1 << 0;
    /// Fluxbox's `DECORM_HANDLE` bit.
    const HANDLE_BIT: i64 = 1 << 1;

    /// Reads a preset (`NORMAL`, `TINY`…) or a mask: decimal, `0x` hex or
    /// `0` octal, as `strtol` reads it.
    fn parse(text: &str) -> Option<Self> {
        let preset = |has_titlebar, has_handle| {
            Some(Self {
                has_titlebar,
                has_handle,
            })
        };
        match text.to_ascii_uppercase().as_str() {
            "NORMAL" | "FULL" => preset(true, true),
            "TINY" | "TOOL" => preset(true, false),
            "NONE" | "BORDER" | "TAB" => preset(false, false),
            _ => {
                let (digits, radix) = if let Some(hex) =
                    text.strip_prefix("0x").or_else(|| text.strip_prefix("0X"))
                {
                    (hex, 16)
                } else if text.len() > 1
                    && let Some(octal) = text.strip_prefix('0')
                {
                    (octal, 8)
                } else {
                    (text, 10)
                };
                let mask = i64::from_str_radix(digits, radix).ok()?;
                preset(mask & Self::TITLEBAR_BIT != 0, mask & Self::HANDLE_BIT != 0)
            }
        }
    }
}

/// An `init` file being converted.
#[derive(Debug)]
pub(crate) struct InitFile {
    /// Its resources.
    resources: Resources,
    /// The settings converted so far.
    document: DocumentMut,
    /// vaht's built-in settings, which converted values are compared to.
    defaults: DocumentMut,
}

impl InitFile {
    /// Converts `resources`.
    pub(crate) fn new(resources: Resources) -> Self {
        let settings = Config::FILES
            .iter()
            .find(|file| file.name == "settings.toml")
            .expect("settings.toml is one of vaht's files");
        Self {
            resources,
            document: DocumentMut::new(),
            defaults: settings
                .defaults
                .parse()
                .expect("vaht's built-in settings are valid TOML"),
        }
    }

    /// Converts the resources, noting in `report` what it leaves out.
    pub(crate) fn convert(mut self, report: &mut Report) -> DocumentMut {
        self.convert_focus(report);
        self.convert_placement(report);
        self.convert_windows(report);
        self.convert_workspaces();
        self.convert_taskbar(report);
        self.report_unused(report);
        self.document
    }

    /// The `[focus]` section.
    fn convert_focus(&mut self, report: &mut Report) {
        if let Some(model) = self.resources.get(&format!("{SCREEN}focusModel")).cloned() {
            let vaht = match model.value.as_str() {
                "ClickFocus" | "ClickToFocus" => Some("click"),
                "MouseFocus" | "SloppyFocus" => Some("sloppy"),
                _ => None,
            };
            match vaht {
                Some(vaht) => self.set(&["focus", "model"], Value::from(vaht)),
                None => report.note(
                    self.resources.path(),
                    model.line,
                    Reason::NotYetInVaht,
                    format!("the focus model {}", model.value),
                ),
            }
        }
        self.convert_flag("autoRaise", &["focus", "auto_raise"]);
        self.convert_flag("clickRaises", &["focus", "raise_on_click"]);
        self.convert_flag("focusNewWindows", &["focus", "focus_new_windows"]);
        if let Some(delay) = self.number("session.autoRaiseDelay") {
            self.set(&["focus", "auto_raise_delay_ms"], Value::from(delay));
        }
    }

    /// The `[placement]` section.
    fn convert_placement(&mut self, report: &mut Report) {
        if let Some(strategy) = self
            .resources
            .get(&format!("{SCREEN}windowPlacement"))
            .cloned()
        {
            let vaht = match strategy.value.as_str() {
                "RowSmartPlacement" => Some("row"),
                "ColSmartPlacement" => Some("column"),
                "RowMinOverlapPlacement" => Some("row-min-overlap"),
                "ColMinOverlapPlacement" => Some("column-min-overlap"),
                "CascadePlacement" => Some("cascade"),
                "UnderMousePlacement" => Some("under-pointer"),
                "CenterPlacement" => Some("center"),
                _ => None,
            };
            match vaht {
                Some(vaht) => self.set(&["placement", "strategy"], Value::from(vaht)),
                None => report.note(
                    self.resources.path(),
                    strategy.line,
                    Reason::NotYetInVaht,
                    format!("the placement {}", strategy.value),
                ),
            }
        }
        if let Some(direction) = self
            .resources
            .value(&format!("{SCREEN}rowPlacementDirection"))
        {
            let vaht = if direction == "RightToLeft" {
                "right-to-left"
            } else {
                "left-to-right"
            };
            self.set(&["placement", "row_direction"], Value::from(vaht));
        }
        if let Some(direction) = self
            .resources
            .value(&format!("{SCREEN}colPlacementDirection"))
        {
            let vaht = if direction == "BottomToTop" {
                "bottom-to-top"
            } else {
                "top-to-bottom"
            };
            self.set(&["placement", "column_direction"], Value::from(vaht));
        }
    }

    /// The `[windows]` section.
    fn convert_windows(&mut self, report: &mut Report) {
        if let Some(distance) = self.number(&format!("{SCREEN}edgeSnapThreshold")) {
            self.set(&["windows", "edge_snap_distance"], Value::from(distance));
        }
        if let Some(interval) = self.number("session.doubleClickInterval") {
            self.set(&["windows", "double_click_ms"], Value::from(interval));
        }
        for (side, key) in [("left", "titlebar_left"), ("right", "titlebar_right")] {
            let Some(resource) = self
                .resources
                .get(&format!("{SCREEN}titlebar.{side}"))
                .cloned()
            else {
                continue;
            };
            let mut buttons = Array::new();
            for name in resource.value.split_whitespace() {
                match name {
                    "Shade" | "Stick" | "Minimize" | "Maximize" | "Close" => {
                        buttons.push(name.to_ascii_lowercase());
                    }
                    other => report.note(
                        self.resources.path(),
                        resource.line,
                        Reason::NotYetInVaht,
                        format!("the titlebar button {other}"),
                    ),
                }
            }
            self.set(&["windows", "frame", key], Value::Array(buttons));
        }
        self.convert_decoration(report);
    }

    /// `defaultDeco`: whether frames have a handle. Frames without a
    /// titlebar are not in vaht yet.
    fn convert_decoration(&mut self, report: &mut Report) {
        let Some(resource) = self.resources.get(&format!("{SCREEN}defaultDeco")).cloned() else {
            return;
        };
        let Some(decoration) = Decoration::parse(&resource.value) else {
            self.note(
                report,
                resource.line,
                format!("the decoration {}: no preset or mask", resource.value),
            );
            return;
        };
        if !decoration.has_titlebar {
            self.note(
                report,
                resource.line,
                format!("windows without a titlebar ({})", resource.value),
            );
        }
        self.set(
            &["windows", "frame", "handle"],
            Value::from(decoration.has_handle),
        );
    }

    /// The `[workspaces]` section: Fluxbox's count, with its names (a
    /// comma-separated list, often with a trailing comma); unnamed
    /// workspaces get vaht's default names.
    fn convert_workspaces(&mut self) {
        let count = self.number(&format!("{SCREEN}workspaces"));
        let listed: Vec<String> = self
            .resources
            .value(&format!("{SCREEN}workspaceNames"))
            .map(|names| {
                names
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let count = count
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(listed.len());
        if count == 0 {
            return;
        }
        let names: Array = (0..count)
            .map(|index| {
                listed
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("Workspace {}", index + 1))
            })
            .collect();
        self.set(&["workspaces", "names"], Value::Array(names));
    }

    /// The `[taskbar]` section, from the toolbar's and iconbar's resources.
    fn convert_taskbar(&mut self, report: &mut Report) {
        let toolbar = |name: &str| format!("{SCREEN}toolbar.{name}");
        self.convert_flag("toolbar.visible", &["taskbar", "enabled"]);
        if let Some(placement) = self.resources.get(&toolbar("placement")).cloned() {
            let value = placement.value.to_ascii_lowercase();
            let edge = ["top", "bottom"]
                .into_iter()
                .find(|edge| value.starts_with(edge));
            match edge {
                Some(edge) => {
                    self.set(&["taskbar", "edge"], Value::from(edge));
                    let align = ["left", "center", "right"]
                        .into_iter()
                        .find(|align| value.ends_with(align))
                        .unwrap_or("center");
                    self.set(&["taskbar", "align"], Value::from(align));
                }
                None => self.note(
                    report,
                    placement.line,
                    format!("a taskbar along the {} side", placement.value),
                ),
            }
        }
        if let Some(covers) = self.resources.value(&toolbar("maxOver")) {
            let reserves = !covers.eq_ignore_ascii_case("true");
            self.set(&["taskbar", "reserve_space"], Value::from(reserves));
        }
        if let Some(width) = self.resources.get(&toolbar("widthPercent")).cloned() {
            match width.value.parse::<i64>() {
                Ok(percent @ 1..=100) => {
                    self.set(&["taskbar", "width_percent"], Value::from(percent));
                }
                Ok(_) | Err(_) => self.note(
                    report,
                    width.line,
                    format!("the toolbar width {}", width.value),
                ),
            }
        }
        self.convert_flag("toolbar.autoHide", &["taskbar", "auto_hide"]);
        if let Some(delay) = self.number(&toolbar("autoHideDelay")) {
            self.set(&["taskbar", "auto_hide_delay_ms"], Value::from(delay));
        }
        if let Some(tools) = self.resources.get(&toolbar("tools")).cloned() {
            let mut widgets = Array::new();
            for tool in tools
                .value
                .split(',')
                .map(str::trim)
                .filter(|tool| !tool.is_empty())
            {
                let widget = match tool.to_ascii_lowercase().as_str() {
                    "prevworkspace" => Some("workspace-previous"),
                    "workspacename" => Some("workspace-name"),
                    "nextworkspace" => Some("workspace-next"),
                    "iconbar" => Some("window-list"),
                    "clock" => Some("clock"),
                    _ => None,
                };
                match widget {
                    Some(widget) => widgets.push(widget),
                    None => self.note(report, tools.line, format!("the toolbar tool {tool}")),
                }
            }
            self.set(&["taskbar", "widgets"], Value::Array(widgets));
        }
        self.convert_clock(report);
        self.convert_window_list(report);
        for (name, default) in [
            ("height", "0"),
            ("onhead", "1"),
            ("autoRaise", "false"),
            ("layer", "Dock"),
        ] {
            self.resources.accept_default(&toolbar(name), default);
        }
    }

    /// The clock's format, from `strftimeFormat`, if vaht reads it.
    fn convert_clock(&mut self, report: &mut Report) {
        let Some(format) = self
            .resources
            .get(&format!("{SCREEN}strftimeFormat"))
            .cloned()
        else {
            return;
        };
        let sample = jiff::civil::date(2026, 1, 1)
            .at(12, 0, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC);
        let is_readable =
            sample.is_ok_and(|sample| jiff::fmt::strtime::format(&format.value, &sample).is_ok());
        if is_readable {
            self.set(&["taskbar", "clock", "format"], Value::from(format.value));
        } else {
            report.note(
                self.resources.path(),
                format.line,
                Reason::Unreadable,
                format!("the clock format {}", format.value),
            );
        }
    }

    /// The window list, from the iconbar's mode and width.
    fn convert_window_list(&mut self, report: &mut Report) {
        if let Some(mode) = self
            .resources
            .get(&format!("{SCREEN}iconbar.mode"))
            .cloned()
        {
            let (matcher, losses) = ClientPattern::new(&mode.value).matcher();
            for (reason, lost) in losses {
                report.note(self.resources.path(), mode.line, reason, lost);
            }
            self.set(
                &["taskbar", "window_list", "show"],
                Value::InlineTable(matcher),
            );
        }
        if let Some(width) = self.number(&format!("{SCREEN}iconbar.iconWidth")) {
            let mut entry_width = toml_edit::InlineTable::new();
            entry_width.insert("max", Value::from(width));
            self.set(
                &["taskbar", "window_list", "entry_width"],
                Value::InlineTable(entry_width),
            );
        }
        self.resources
            .accept_default(&format!("{SCREEN}iconbar.alignment"), "Relative");
        self.convert_flag("iconbar.usePixmap", &["taskbar", "window_list", "icons"]);
        // The theme's taskbar.window-list.entry padding is half of it each
        // side; another value is reported, for the theme to change.
        self.resources
            .accept_default(&format!("{SCREEN}iconbar.iconTextPadding"), "10");
    }

    /// Notes that what `init` says on line `line` is not in vaht yet.
    fn note(&self, report: &mut Report, line: usize, what: String) {
        report.note(self.resources.path(), line, Reason::NotYetInVaht, what);
    }

    /// Converts a `true`/`false` screen resource to the setting at `path`.
    fn convert_flag(&mut self, resource: &str, path: &[&str]) {
        if let Some(value) = self.resources.value(&format!("{SCREEN}{resource}")) {
            let is_on = value.eq_ignore_ascii_case("true");
            self.set(path, Value::from(is_on));
        }
    }

    /// Returns the resource `name` as a whole number, if it is one.
    fn number(&mut self, name: &str) -> Option<i64> {
        self.resources
            .value(name)
            .and_then(|value| value.parse().ok())
    }

    /// Sets the setting at `path` (sections, then the key) to `value`,
    /// unless that is vaht's default anyway.
    fn set(&mut self, path: &[&str], value: Value) {
        let Some((key, sections)) = path.split_last() else {
            return;
        };
        let default = sections
            .iter()
            .try_fold(self.defaults.as_table(), |table, section| {
                table.get(section).and_then(Item::as_table)
            })
            .and_then(|table| table.get(key))
            .and_then(Item::as_value);
        if default.is_some_and(|default| Self::same(default, &value)) {
            return;
        }
        let mut table = self.document.as_table_mut();
        for section in sections {
            table = table
                .entry(section)
                .or_insert_with(|| {
                    // A section with only subsections needs no header.
                    let mut new = Table::new();
                    new.set_implicit(true);
                    Item::Table(new)
                })
                .as_table_mut()
                .expect("settings sections are tables");
        }
        table.insert(key, Item::Value(value));
    }

    /// Returns whether two values are equal, whatever their formatting.
    fn same(first: &Value, second: &Value) -> bool {
        let bare = |value: &Value| {
            let mut value = value.clone();
            value.decor_mut().clear();
            if let Value::Array(array) = &mut value {
                array.fmt();
            }
            value.to_string()
        };
        bare(first) == bare(second)
    }

    /// Reports every resource the conversion did not use.
    fn report_unused(&self, report: &mut Report) {
        for resource in self.resources.unused() {
            let short = resource.name.strip_prefix(SCREEN).unwrap_or(&resource.name);
            let dropped = DROPPED
                .iter()
                .find(|(fragment, _)| short.contains(fragment));
            let is_opaque = short.ends_with("alpha") && resource.value == "255";
            let (reason, why) = match (dropped, is_opaque) {
                (Some((_, why)), _) => (Reason::Dropped, *why),
                (None, true) => (Reason::Dropped, "fully opaque is the default"),
                (None, false) => (Reason::NotYetInVaht, "no vaht setting yet"),
            };
            report.note(
                self.resources.path(),
                resource.line,
                reason,
                format!("{} = {} ({why})", resource.name, resource.value),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn only_settings_that_differ_from_the_defaults_are_written() {
        let init = "session.screen0.focusModel:\tClickFocus
session.screen0.windowPlacement:\tRowMinOverlapPlacement
session.screen0.titlebar.right:\tShade Minimize Maximize Close
session.screen0.workspaces:\t4
session.screen0.workspaceNames:\tWorkspace 1,Workspace 2,Workspace 3,Workspace 4,
session.screen0.edgeSnapThreshold:\t10
session.screen0.tabs.usePixmap:\ttrue
session.screen0.toolbar.alpha:\t255
session.screen0.toolbar.widthPercent:\t80
";
        let mut report = Report::default();
        let document =
            InitFile::new(Resources::parse(Path::new("init"), init)).convert(&mut report);
        assert_eq!(
            document.to_string(),
            "[placement]\nstrategy = \"row-min-overlap\"\n\n[windows.frame]\n\
             titlebar_right = [\"shade\", \"minimize\", \"maximize\", \"close\"]\n\n\
             [taskbar]\nwidth_percent = 80\n"
        );
        let reasons: Vec<_> = report
            .notes()
            .iter()
            .map(|note| (note.line, note.reason))
            .collect();
        assert_eq!(
            reasons,
            [(7, Reason::Dropped), (8, Reason::Dropped)],
            "the tabs and the opaque alpha"
        );
    }

    #[test]
    fn the_decoration_says_whether_frames_have_a_handle() {
        let convert = |value: &str| {
            let init = format!("session.screen0.defaultDeco:\t{value}\n");
            let mut report = Report::default();
            let document =
                InitFile::new(Resources::parse(Path::new("init"), &init)).convert(&mut report);
            (document.to_string(), report.notes().len())
        };
        assert_eq!(
            convert("0x3fd"),
            (String::new(), 0),
            "everything but the handle: vaht's default"
        );
        let shown = "[windows.frame]\nhandle = true\n".to_owned();
        assert_eq!(convert("NORMAL"), (shown.clone(), 0));
        assert_eq!(convert("1023"), (shown, 0), "all ten bits, in decimal");
        assert_eq!(convert("TINY"), (String::new(), 0));
        assert_eq!(convert("NONE"), (String::new(), 1), "no titlebar: reported");
        assert_eq!(convert("sideways"), (String::new(), 1), "not a decoration");
    }

    #[test]
    fn the_toolbar_becomes_the_taskbar() {
        let init = "session.screen0.toolbar.placement:\tTopLeft
session.screen0.toolbar.tools:\tprevworkspace, iconbar, systemtray, clock
session.screen0.toolbar.maxOver:\ttrue
session.screen0.strftimeFormat:\t%H:%M:%S
session.screen0.iconbar.mode:\t{static groups} (workspace) (minimized=no)
session.screen0.iconbar.iconWidth:\t200
session.screen0.iconbar.usePixmap:\tfalse
session.screen0.toolbar.widthPercent:\t100
";
        let mut report = Report::default();
        let document =
            InitFile::new(Resources::parse(Path::new("init"), init)).convert(&mut report);
        assert_eq!(
            document.to_string(),
            "[taskbar]\nedge = \"top\"\nalign = \"left\"\nreserve_space = false\n\
             widgets = [\"workspace-previous\", \"window-list\", \"clock\"]\n\n\
             [taskbar.clock]\nformat = \"%H:%M:%S\"\n\n\
             [taskbar.window_list]\nshow = { workspace = \"current\", minimized = false }\n\
             entry_width = { max = 200 }\nicons = false\n"
        );
        let notes: Vec<String> = report.notes().iter().map(ToString::to_string).collect();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("the toolbar tool systemtray"),
            "{notes:?}"
        );
    }
}

//! Converting Fluxbox's `init` to `settings.toml` (02 §3).
//!
//! Each resource vaht has a setting for becomes that setting, written only
//! when it differs from vaht's default: the built-in `settings.toml` is read
//! for the defaults, so they are written down in one place. Every other
//! resource is reported: dropped (tabs, the slit, X11 tuning) or not in vaht
//! yet.

use toml_edit::{Array, DocumentMut, Item, Table, Value};
use vaht_compositor::Config;

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
        let [_, settings] = Config::FILES;
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
session.screen0.toolbar.widthPercent:\t100
";
        let mut report = Report::default();
        let document =
            InitFile::new(Resources::parse(Path::new("init"), init)).convert(&mut report);
        assert_eq!(
            document.to_string(),
            "[placement]\nstrategy = \"row-min-overlap\"\n\n[windows.frame]\n\
             titlebar_right = [\"shade\", \"minimize\", \"maximize\", \"close\"]\n"
        );
        let reasons: Vec<_> = report
            .notes()
            .iter()
            .map(|note| (note.line, note.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                (7, Reason::Dropped),
                (8, Reason::Dropped),
                (9, Reason::NotYetInVaht)
            ]
        );
    }
}

//! Converting fluxland's `session.input.*` resources (libinput settings)
//! to `settings.toml`'s `[input]` (02 §3).
//!
//! `session.input.<key>` becomes `[input.all]`, `.touchpad.` and
//! `.pointer.` their sections, and `.device.<Name>.` becomes
//! `[input.device."<Name>"]`: fluxland's name with each character but
//! letters and digits made `_`, which vaht matches as well as the real
//! name. Keys go from `camelCase` to `snake_case`, and values to vaht's.

use toml_edit::{DocumentMut, Item, Table, Value};

use crate::report::{Reason, Report};
use crate::resources::Resources;

/// The prefix of fluxland's input resources.
const PREFIX: &str = "session.input.";

/// Converts the `session.input.*` resources in `resources` into
/// `document`'s `[input]`, noting in `report` what vaht cannot take.
pub(crate) fn convert(resources: &mut Resources, document: &mut DocumentMut, report: &mut Report) {
    for name in resources.names_with_prefix(PREFIX) {
        let Some(resource) = resources.get(&name).cloned() else {
            continue;
        };
        let rest = name.strip_prefix(PREFIX).unwrap_or_default();
        let converted = scope_and_key(rest).and_then(|(scope, key)| {
            setting(key, &resource.value).map(|(key, value)| (scope, key, value))
        });
        match converted {
            Some((scope, key, value)) => {
                section(document, &scope).insert(key, Item::Value(value));
            }
            None => report.note(
                resources.path(),
                resource.line,
                Reason::NotYetInVaht,
                format!("{} = {}", resource.name, resource.value),
            ),
        }
    }
}

/// Where a setting goes under `[input]`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    /// `[input.all]`, `[input.touchpad]` or `[input.pointer]`.
    Section(&'static str),
    /// `[input.device."<name>"]`.
    Device(String),
}

/// Returns where the resource after `session.input.` (`rest`) goes, and
/// its key.
fn scope_and_key(rest: &str) -> Option<(Scope, &str)> {
    if let Some(key) = rest.strip_prefix("touchpad.") {
        return Some((Scope::Section("touchpad"), key));
    }
    if let Some(key) = rest.strip_prefix("pointer.") {
        return Some((Scope::Section("pointer"), key));
    }
    if let Some(device) = rest.strip_prefix("device.") {
        let (name, key) = device.rsplit_once('.')?;
        return (!name.is_empty()).then(|| (Scope::Device(name.to_owned()), key));
    }
    (!rest.contains('.')).then_some((Scope::Section("all"), rest))
}

/// Returns `[input]`'s table for `scope` in `document`, made if new.
fn section<'document>(document: &'document mut DocumentMut, scope: &Scope) -> &'document mut Table {
    let input = implicit_table(document.as_table_mut(), "input");
    match scope {
        Scope::Section(name) => plain_table(input, name),
        Scope::Device(name) => plain_table(implicit_table(input, "device"), name),
    }
}

/// Returns `parent`'s table `key`, made with no header of its own if new.
fn implicit_table<'table>(parent: &'table mut Table, key: &str) -> &'table mut Table {
    parent
        .entry(key)
        .or_insert_with(|| {
            let mut table = Table::new();
            table.set_implicit(true);
            Item::Table(table)
        })
        .as_table_mut()
        .expect("input sections are tables")
}

/// Returns `parent`'s table `key`, made if new.
fn plain_table<'table>(parent: &'table mut Table, key: &str) -> &'table mut Table {
    parent
        .entry(key)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .expect("input sections are tables")
}

/// Returns fluxland's setting `key` (any case) of `value` as vaht's key
/// and value; none if vaht has no such setting or value.
fn setting(key: &str, value: &str) -> Option<(&'static str, Value)> {
    let key = key.to_ascii_lowercase();
    let boolean = |vaht: &'static str| boolean(value).map(|is_on| (vaht, Value::from(is_on)));
    let choice = |vaht: &'static str, choices: &[(&str, &'static str)]| {
        choices
            .iter()
            .find(|(fluxland, _)| fluxland.eq_ignore_ascii_case(value))
            .map(|(_, converted)| (vaht, Value::from(*converted)))
    };
    match key.as_str() {
        "tap" => boolean("tap"),
        "tapdrag" => boolean("tap_drag"),
        "tapdraglock" => boolean("tap_drag_lock"),
        "naturalscroll" => boolean("natural_scroll"),
        "lefthanded" => boolean("left_handed"),
        "disablewhiletyping" => boolean("disable_while_typing"),
        "middleemulation" => boolean("middle_emulation"),
        "tapbuttonmap" => choice("tap_button_map", &[("lrm", "lrm"), ("lmr", "lmr")]),
        "accelprofile" => choice(
            "accel_profile",
            &[("flat", "flat"), ("adaptive", "adaptive")],
        ),
        "accelspeed" => value
            .parse::<f64>()
            .ok()
            .filter(|speed| (-1.0..=1.0).contains(speed))
            .map(|speed| ("accel_speed", Value::from(speed))),
        "scrollmethod" => choice(
            "scroll_method",
            &[
                ("none", "none"),
                ("twoFinger", "two-finger"),
                ("edge", "edge"),
                ("button", "button"),
            ],
        ),
        "scrollbutton" => value
            .parse::<i64>()
            .ok()
            .filter(|button| *button > 0)
            .map(|button| ("scroll_button", Value::from(button))),
        "clickmethod" => choice(
            "click_method",
            &[
                ("buttonAreas", "button-areas"),
                ("clickFinger", "click-finger"),
            ],
        ),
        "sendevents" => choice(
            "send_events",
            &[
                ("enabled", "enabled"),
                ("disabled", "disabled"),
                ("disabledOnExternalMouse", "disabled-on-external-mouse"),
            ],
        ),
        _ => None,
    }
}

/// Returns fluxland's boolean `value` (`true`, `yes`, `1`, and their
/// opposites), if it is one.
fn boolean(value: &str) -> Option<bool> {
    let value = value.to_ascii_lowercase();
    match value.as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Converts `init`; returns the settings and the report's notes.
    fn convert_init(init: &str) -> (String, Vec<String>) {
        let mut resources = Resources::parse(std::path::Path::new("init"), init);
        let mut document = DocumentMut::new();
        let mut report = Report::default();
        convert(&mut resources, &mut document, &mut report);
        let notes = report.notes().iter().map(ToString::to_string).collect();
        (document.to_string(), notes)
    }

    #[test]
    fn each_scope_goes_to_its_section() {
        let (settings, notes) = convert_init(
            "session.input.naturalScroll: true\n\
             session.input.touchpad.tap: yes\n\
             session.input.touchpad.tapButtonMap: lmr\n\
             session.input.touchpad.scrollMethod: twoFinger\n\
             session.input.touchpad.clickMethod: clickFinger\n\
             session.input.pointer.accelProfile: flat\n\
             session.input.pointer.accelSpeed: -0.5\n\
             session.input.device.Logitech_USB_Trackball.scrollMethod: button\n\
             session.input.device.Logitech_USB_Trackball.scrollButton: 276\n\
             session.input.device.Logitech_USB_Trackball.leftHanded: 1\n",
        );
        assert_eq!(notes, Vec::<String>::new());
        let parsed: DocumentMut = settings.parse().expect("valid TOML");
        let input = &parsed["input"];
        assert_eq!(input["all"]["natural_scroll"].as_bool(), Some(true));
        assert_eq!(input["touchpad"]["tap"].as_bool(), Some(true));
        assert_eq!(input["touchpad"]["tap_button_map"].as_str(), Some("lmr"));
        assert_eq!(
            input["touchpad"]["scroll_method"].as_str(),
            Some("two-finger")
        );
        assert_eq!(
            input["touchpad"]["click_method"].as_str(),
            Some("click-finger")
        );
        assert_eq!(input["pointer"]["accel_profile"].as_str(), Some("flat"));
        assert_eq!(input["pointer"]["accel_speed"].as_float(), Some(-0.5));
        let trackball = &input["device"]["Logitech_USB_Trackball"];
        assert_eq!(trackball["scroll_method"].as_str(), Some("button"));
        assert_eq!(trackball["scroll_button"].as_integer(), Some(276));
        assert_eq!(trackball["left_handed"].as_bool(), Some(true));
    }

    #[test]
    fn what_vaht_cannot_take_is_reported() {
        let (_, notes) = convert_init(
            "session.input.touchpad.clickMethod: none\n\
             session.input.accelSpeed: 3\n\
             session.input.touchpad.sparkle: true\n",
        );
        assert_eq!(notes.len(), 3, "{notes:?}");
    }
}

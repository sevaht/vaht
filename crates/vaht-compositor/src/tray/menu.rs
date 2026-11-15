//! Tray items' menus (`com.canonical.dbusmenu`), which vaht shows.
//!
//! Most tray items leave their menu to the tray: they serve its items on
//! the bus, and the tray asks for them as the menu opens (`AboutToShow`,
//! then `GetLayout`), shows them with its own menus, and tells the item
//! which entry was chosen (`Event`, `clicked`). [`TrayMenu`] is a menu as
//! plain values, read from the layout [`TrayMenu::from_layout`] is given.

use std::collections::HashMap;

use smithay::utils::{Logical, Point};
use zbus::OwnedVariant;

use super::TrayItemKey;

/// A menu entry as `GetLayout` gives it: its id, its properties, and its
/// children, each another entry in a variant.
pub(crate) type Layout = (i32, HashMap<String, OwnedVariant>, Vec<OwnedVariant>);

/// A tray item's menu: what it serves under `com.canonical.dbusmenu`.
#[zbus::proxy(interface = "com.canonical.dbusmenu")]
pub(crate) trait DbusMenu {
    /// The entries under `parent_id` (0: the whole menu), `recursion_depth`
    /// levels deep (-1: all), with `property_names` (none: all); and the
    /// layout's revision.
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: &[&str],
    ) -> zbus::Result<(u32, Layout)>;

    /// Tells the item the entry `id` is about to show its children, so it
    /// may fill them in; returns whether the layout changed.
    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;

    /// Tells the item the entry `id` had `event_id` (`clicked`), with
    /// `data`, at `timestamp`.
    fn event(
        &self,
        id: i32,
        event_id: &str,
        data: &zbus::Variant<'_>,
        timestamp: u32,
    ) -> zbus::Result<()>;
}

/// A tray item's menu, read, and where to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrayMenu {
    /// The item it belongs to.
    pub(crate) item: TrayItemKey,
    /// Where the click that opened it was, globally.
    pub(crate) at: Point<i32, Logical>,
    /// Its entries, top to bottom.
    pub(crate) entries: Vec<TrayMenuEntry>,
}

/// One entry of a tray item's menu. Hidden entries are left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrayMenuEntry {
    /// An entry that does something when chosen.
    Choice {
        /// The item's id for it, which choosing it reports.
        id: i32,
        /// What it says, its mnemonic marks taken out.
        label: String,
        /// Whether it can be chosen.
        is_enabled: bool,
        /// Its check mark or radio button, if it has one: whether it is on.
        mark: Option<bool>,
    },
    /// An entry that opens more entries.
    Submenu {
        /// What it says.
        label: String,
        /// Its entries.
        entries: Vec<Self>,
    },
    /// A line between entries.
    Separator,
}

impl TrayMenu {
    /// Returns the menu of the tray item `item` the root entry `layout`
    /// describes, opened by a click at `at`.
    pub(crate) fn from_layout(item: TrayItemKey, at: Point<i32, Logical>, layout: Layout) -> Self {
        let (_, _, children) = layout;
        Self {
            item,
            at,
            entries: entries_of(children),
        }
    }
}

/// Returns the visible entries among `children`, each an entry's layout in
/// a variant; children that are not are skipped.
fn entries_of(children: Vec<OwnedVariant>) -> Vec<TrayMenuEntry> {
    children
        .into_iter()
        .filter_map(|child| Layout::try_from(child).ok())
        .filter_map(entry_of)
        .collect()
}

/// Returns the entry `layout` describes, unless it is hidden.
fn entry_of((id, properties, children): Layout) -> Option<TrayMenuEntry> {
    let text = |name: &str| {
        properties
            .get(name)
            .and_then(|value| String::try_from(value.clone()).ok())
    };
    let flag = |name: &str, default: bool| {
        properties
            .get(name)
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(default)
    };
    if !flag("visible", true) {
        return None;
    }
    if text("type").as_deref() == Some("separator") {
        return Some(TrayMenuEntry::Separator);
    }
    let label = without_mnemonics(&text("label").unwrap_or_default());
    if text("children-display").as_deref() == Some("submenu") || !children.is_empty() {
        return Some(TrayMenuEntry::Submenu {
            label,
            entries: entries_of(children),
        });
    }
    let toggles = matches!(text("toggle-type").as_deref(), Some("checkmark" | "radio"));
    let state = properties
        .get("toggle-state")
        .and_then(|value| i32::try_from(value).ok());
    Some(TrayMenuEntry::Choice {
        id,
        label,
        is_enabled: flag("enabled", true),
        mark: toggles.then_some(state == Some(1)),
    })
}

/// Returns `label` without its mnemonic marks: an underscore marks the
/// letter after it, and two stand for one.
fn without_mnemonics(label: &str) -> String {
    let mut shown = String::with_capacity(label.len());
    let mut characters = label.chars();
    while let Some(character) = characters.next() {
        if character == '_' {
            if let Some(next) = characters.next() {
                shown.push(next);
            }
        } else {
            shown.push(character);
        }
    }
    shown
}

#[cfg(test)]
mod tests {
    use zbus::Variant;

    use super::*;

    /// Returns `value` as an owned variant.
    fn owned(value: &Variant<'_>) -> OwnedVariant {
        value.try_to_owned().expect("a plain value")
    }

    /// An entry's layout, `id` with `properties` and `children`, in a
    /// variant, as `GetLayout` nests them.
    fn entry(
        id: i32,
        properties: &[(&str, Variant<'_>)],
        children: Vec<OwnedVariant>,
    ) -> OwnedVariant {
        let properties: HashMap<String, OwnedVariant> = properties
            .iter()
            .map(|(name, value)| ((*name).to_owned(), owned(value)))
            .collect();
        let layout: Layout = (id, properties, children);
        owned(&Variant::from(layout))
    }

    /// The root entry holding `children`.
    fn root(children: Vec<OwnedVariant>) -> Layout {
        (0, HashMap::new(), children)
    }

    /// The menu `children` make.
    fn menu(children: Vec<OwnedVariant>) -> Vec<TrayMenuEntry> {
        TrayMenu::from_layout(TrayItemKey(1), Point::from((0, 0)), root(children)).entries
    }

    #[test]
    fn entries_are_choices_separators_and_submenus() {
        let entries = menu(vec![
            entry(1, &[("label", Variant::from("_Open"))], Vec::new()),
            entry(2, &[("type", Variant::from("separator"))], Vec::new()),
            entry(
                3,
                &[
                    ("label", Variant::from("More")),
                    ("children-display", Variant::from("submenu")),
                ],
                vec![entry(4, &[("label", Variant::from("Quit"))], Vec::new())],
            ),
        ]);
        assert_eq!(
            entries,
            [
                TrayMenuEntry::Choice {
                    id: 1,
                    label: "Open".to_owned(),
                    is_enabled: true,
                    mark: None,
                },
                TrayMenuEntry::Separator,
                TrayMenuEntry::Submenu {
                    label: "More".to_owned(),
                    entries: vec![TrayMenuEntry::Choice {
                        id: 4,
                        label: "Quit".to_owned(),
                        is_enabled: true,
                        mark: None,
                    }],
                },
            ]
        );
    }

    #[test]
    fn hidden_entries_are_left_out_and_disabled_ones_kept() {
        let entries = menu(vec![
            entry(
                1,
                &[
                    ("label", Variant::from("Hidden")),
                    ("visible", Variant::from(false)),
                ],
                Vec::new(),
            ),
            entry(
                2,
                &[
                    ("label", Variant::from("Greyed")),
                    ("enabled", Variant::from(false)),
                ],
                Vec::new(),
            ),
        ]);
        assert_eq!(
            entries,
            [TrayMenuEntry::Choice {
                id: 2,
                label: "Greyed".to_owned(),
                is_enabled: false,
                mark: None,
            }]
        );
    }

    #[test]
    fn a_toggle_shows_whether_it_is_on() {
        let toggle = |id, state: i32| {
            entry(
                id,
                &[
                    ("label", Variant::from("Mute")),
                    ("toggle-type", Variant::from("checkmark")),
                    ("toggle-state", Variant::from(state)),
                ],
                Vec::new(),
            )
        };
        let marks: Vec<_> = menu(vec![toggle(1, 1), toggle(2, 0)])
            .into_iter()
            .map(|entry| match entry {
                TrayMenuEntry::Choice { mark, .. } => mark,
                other @ (TrayMenuEntry::Submenu { .. } | TrayMenuEntry::Separator) => {
                    panic!("not a choice: {other:?}")
                }
            })
            .collect();
        assert_eq!(marks, [Some(true), Some(false)]);
    }

    #[test]
    fn mnemonic_marks_are_taken_out_and_doubled_ones_kept_once() {
        assert_eq!(without_mnemonics("_File"), "File");
        assert_eq!(without_mnemonics("Save __as"), "Save _as");
        assert_eq!(without_mnemonics("trailing_"), "trailing");
    }
}

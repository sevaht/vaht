//! Splits converted settings in two: what is shown and how it is arranged
//! goes to a layout, the rest (behavior) stays in `settings.toml` (02 §10).
//!
//! What is presentation must match what the compositor lets a layout set;
//! the end-to-end tests load a converted setup to keep the two in step.

use toml_edit::{DocumentMut, Item, Table};

/// What a layout may set, by dotted path in `settings.toml`.
const PRESENTATION: [&str; 11] = [
    "taskbar",
    "windows.frame",
    "windows.focused_alpha",
    "windows.unfocused_alpha",
    "windows.snapping.layout_picker",
    "windows.snapping.picker_size",
    "windows.snapping.picker_columns",
    "windows.snapping.layouts",
    "blur",
    "menus.window_icons",
    "menus.infer_icons",
];

/// Takes the presentation settings out of `settings` and returns them as the
/// document of a layout: empty if `settings` has none.
pub(crate) fn take_layout(settings: &mut DocumentMut) -> DocumentMut {
    let mut layout = DocumentMut::new();
    for path in PRESENTATION {
        let parts: Vec<&str> = path.split('.').collect();
        if let Some(item) = remove(settings.as_table_mut(), &parts) {
            insert(layout.as_table_mut(), &parts, item);
        }
    }
    prune(settings.as_table_mut());
    layout
}

/// Removes the item at `path` from `table`.
fn remove(table: &mut Table, path: &[&str]) -> Option<Item> {
    match path {
        [] => None,
        [last] => table.remove(last),
        [first, rest @ ..] => remove(table.get_mut(first)?.as_table_mut()?, rest),
    }
}

/// Puts `item` at `path` in `table`, making the tables on the way.
fn insert(table: &mut Table, path: &[&str], item: Item) {
    match path {
        [] => {}
        [last] => {
            table.insert(last, item);
        }
        [first, rest @ ..] => {
            if !table.contains_key(first) {
                let mut inner = Table::new();
                inner.set_implicit(true);
                table.insert(first, Item::Table(inner));
            }
            if let Some(inner) = table.get_mut(first).and_then(Item::as_table_mut) {
                insert(inner, rest, item);
            }
        }
    }
}

/// Removes the tables in `table` that nothing is left in.
fn prune(table: &mut Table) {
    let keys: Vec<String> = table.iter().map(|(key, _)| key.to_owned()).collect();
    for key in keys {
        if let Some(inner) = table.get_mut(&key).and_then(Item::as_table_mut) {
            prune(inner);
            if inner.is_empty() {
                table.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_moves_to_the_layout_and_behavior_stays() {
        let mut settings: DocumentMut = "[taskbar]\nrows = 2\n[windows]\nedge_snap_distance = 5\n\
             focused_alpha = 200\n[windows.frame]\nhandle = false\n[focus]\nmodel = \"sloppy\"\n"
            .parse()
            .expect("TOML");
        let layout = take_layout(&mut settings);
        assert_eq!(
            layout.to_string().trim(),
            "[taskbar]\nrows = 2\n\n[windows]\nfocused_alpha = 200\n[windows.frame]\nhandle = false"
        );
        assert_eq!(
            settings.to_string().trim(),
            "[windows]\nedge_snap_distance = 5\n[focus]\nmodel = \"sloppy\""
        );
    }

    #[test]
    fn settings_with_no_presentation_make_an_empty_layout() {
        let mut settings: DocumentMut = "[focus]\nmodel = \"sloppy\"\n".parse().expect("TOML");
        assert!(take_layout(&mut settings).as_table().is_empty());
    }
}

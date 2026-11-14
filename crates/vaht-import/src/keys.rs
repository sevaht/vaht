//! Converting Fluxbox's `keys` file to `bindings.toml` (02 §6).
//!
//! Each line is `[mode:] [context] [modifiers] [Double] key-or-button
//! :command`, where a key may be a chain (`Mod4 x Mod4 t`). Key bindings
//! go to `[keys]`, or `[key_modes.<mode>]`, mouse bindings to
//! `[mouse.<context>]`. When
//! a chord is bound twice, the later binding wins, as in Fluxbox, and the
//! earlier one is reported.

use std::collections::HashMap;
use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, Value};

use crate::command::{FluxboxCommand, Untranslated};
use crate::report::{Reason, Report};

/// A Fluxbox `keys` file being converted.
#[derive(Debug, Clone, Copy)]
pub(crate) struct KeysFile<'text> {
    /// Where it was read from.
    path: &'text Path,
    /// Its contents.
    text: &'text str,
}

/// Where a binding goes in `bindings.toml`: the table, and its key there.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Slot {
    /// The key mode, `None` for `[keys]` and the mouse.
    mode: Option<String>,
    /// `None` for keys, or the mouse context.
    context: Option<&'static str>,
    /// The chord or trigger, as vaht writes it.
    trigger: String,
}

impl<'text> KeysFile<'text> {
    /// Fluxbox's modifier names and vaht's, in the order vaht writes them.
    const MODIFIERS: [(&str, &str); 4] = [
        ("mod4", "Super"),
        ("control", "Ctrl"),
        ("mod1", "Alt"),
        ("shift", "Shift"),
    ];

    /// Fluxbox's mouse contexts and vaht's; `None` where vaht has none.
    const CONTEXTS: [(&str, Option<&str>); 9] = [
        ("ondesktop", Some("desktop")),
        ("ontoolbar", Some("taskbar")),
        ("onwindow", Some("window")),
        ("ontitlebar", Some("titlebar")),
        ("onleftgrip", Some("grip-left")),
        ("onrightgrip", Some("grip-right")),
        ("onwindowborder", Some("border")),
        ("ontab", None),
        ("onslit", None),
    ];

    /// The buttons Fluxbox numbers, by number.
    const BUTTONS: [(u8, &str); 7] = [
        (1, "Left"),
        (2, "Middle"),
        (3, "Right"),
        (4, "ScrollUp"),
        (5, "ScrollDown"),
        (8, "Back"),
        (9, "Forward"),
    ];

    /// A `keys` file read from `path`.
    pub(crate) const fn new(path: &'text Path, text: &'text str) -> Self {
        Self { path, text }
    }

    /// Converts the file, noting in `report` what it leaves out.
    pub(crate) fn convert(self, report: &mut Report) -> DocumentMut {
        let mut document = DocumentMut::new();
        // Keys first, then mouse contexts, whatever order the file has.
        Self::table(document.as_table_mut(), "keys");
        Self::table(document.as_table_mut(), "key_modes").set_implicit(true);
        Self::table(document.as_table_mut(), "mouse").set_implicit(true);
        let mut bound_on: HashMap<Slot, usize> = HashMap::new();
        for (index, line) in self.text.lines().enumerate() {
            let number = index + 1;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            match Self::binding(line) {
                Ok((slot, command)) => {
                    let translated = match FluxboxCommand(command).to_vaht() {
                        Ok(translated) => translated,
                        Err(Untranslated { reason, detail }) => {
                            report.note(self.path, number, reason, detail);
                            continue;
                        }
                    };
                    for (reason, detail) in translated.losses {
                        report.note(self.path, number, reason, detail);
                    }
                    if let Some(earlier) = bound_on.insert(slot.clone(), number) {
                        report.note(
                            self.path,
                            earlier,
                            Reason::Dropped,
                            format!(
                                "{} is bound again on line {number}, which wins",
                                slot.trigger
                            ),
                        );
                    }
                    Self::insert(&mut document, &slot, translated.value);
                }
                Err((reason, detail)) => report.note(self.path, number, reason, detail),
            }
        }
        for name in ["keys", "key_modes", "mouse"] {
            if document
                .get(name)
                .and_then(Item::as_table)
                .is_some_and(Table::is_empty)
            {
                document.remove(name);
            }
        }
        document
    }

    /// Splits a binding line into where it goes and its command text. A
    /// line may begin with a key mode's name and a colon (`moving: h
    /// :MoveLeft 10`); `default:` is `[keys]`.
    fn binding(line: &'text str) -> Result<(Slot, &'text str), (Reason, String)> {
        let (mode, line) = match line.split_once(char::is_whitespace) {
            Some((first, rest)) if first.len() > 1 && first.ends_with(':') => {
                let name = &first[..first.len() - 1];
                let mode = (!name.eq_ignore_ascii_case("default")).then(|| name.to_owned());
                (mode, rest.trim_start())
            }
            _ => (None, line),
        };
        let (left, command) = line.split_once(':').ok_or_else(|| {
            (
                Reason::Unreadable,
                format!("no command after a colon: {line}"),
            )
        })?;
        let mut context = None;
        let mut modifiers = Vec::new();
        let mut gesture = "";
        let mut button = None;
        let mut chords = Vec::new();
        for token in left.split_whitespace() {
            let lowercase = token.to_ascii_lowercase();
            if let Some((_, vaht)) = Self::CONTEXTS.iter().find(|(name, _)| *name == lowercase) {
                context = Some(vaht.ok_or_else(|| {
                    (
                        Reason::Dropped,
                        format!("{token} bindings (tabs and the slit are gone)"),
                    )
                })?);
            } else if lowercase == "double" {
                gesture = "Double+";
            } else if let Some((drag, number)) = Self::button_token(&lowercase) {
                if drag {
                    gesture = "Drag+";
                }
                button = Some(
                    Self::button(number)
                        .ok_or_else(|| (Reason::NotYetInVaht, format!("mouse button {number}")))?,
                );
            } else if lowercase == "arg" {
                return Err((
                    Reason::NotYetInVaht,
                    format!("a chain ending in whatever key comes ({left})"),
                ));
            } else if !Self::modifier_token(token, &mut modifiers)? {
                chords.push(format!("{}{token}", Self::modifier_prefix(&mut modifiers)));
            }
        }
        let prefix = Self::modifier_prefix(&mut modifiers);
        let slot = match (button, chords.as_slice()) {
            (Some(_), _) if mode.is_some() => {
                return Err((
                    Reason::NotYetInVaht,
                    format!("a mouse binding in a key mode ({left})"),
                ));
            }
            (Some(button), []) => Slot {
                mode: None,
                context: Some(context.unwrap_or("window")),
                trigger: format!("{prefix}{gesture}{button}"),
            },
            (None, [_, ..]) if context.is_none() && prefix.is_empty() => Slot {
                mode,
                context: None,
                trigger: chords.join(" "),
            },
            _ => return Err((Reason::Unreadable, format!("the binding {left}"))),
        };
        Ok((slot, command))
    }

    /// Converts Fluxbox's key chain `text` (`Mod4 x Mod4 t`, or `None
    /// Escape`) to vaht's (`Super+x Super+t`), if it is one.
    pub(crate) fn chain(text: &str) -> Option<String> {
        let mut modifiers = Vec::new();
        let mut chords = Vec::new();
        for token in text.split_whitespace() {
            let is_modifier = Self::modifier_token(token, &mut modifiers).ok()?;
            if !is_modifier {
                chords.push(format!("{}{token}", Self::modifier_prefix(&mut modifiers)));
            }
        }
        (modifiers.is_empty() && !chords.is_empty()).then(|| chords.join(" "))
    }

    /// Adds the modifier `token` names to `modifiers`, if it names one,
    /// and returns whether it did; `None` names none, and adds nothing.
    fn modifier_token(
        token: &str,
        modifiers: &mut Vec<&'static str>,
    ) -> Result<bool, (Reason, String)> {
        let lowercase = token.to_ascii_lowercase();
        if lowercase == "none" {
            return Ok(true);
        }
        if let Some((_, vaht)) = Self::MODIFIERS.iter().find(|(name, _)| *name == lowercase) {
            modifiers.push(vaht);
            return Ok(true);
        }
        if lowercase.starts_with("mod") {
            return Err((Reason::NotYetInVaht, format!("the modifier {token}")));
        }
        Ok(false)
    }

    /// Returns the `modifiers` gathered as vaht writes them before a key
    /// (`Super+Shift+`), and empties them for the next chord.
    fn modifier_prefix(modifiers: &mut Vec<&'static str>) -> String {
        modifiers.sort_by_key(|modifier| {
            Self::MODIFIERS
                .iter()
                .position(|(_, vaht)| vaht == modifier)
        });
        modifiers
            .drain(..)
            .fold(String::new(), |mut prefix, modifier| {
                prefix.push_str(modifier);
                prefix.push('+');
                prefix
            })
    }

    /// Reads `mouseN`, `clickN` or `moveN`: whether it is a drag, and N.
    fn button_token(lowercase: &str) -> Option<(bool, u8)> {
        let (drag, number) = if let Some(number) = lowercase.strip_prefix("move") {
            (true, number)
        } else {
            (
                false,
                lowercase
                    .strip_prefix("mouse")
                    .or_else(|| lowercase.strip_prefix("click"))?,
            )
        };
        number.parse().ok().map(|number| (drag, number))
    }

    /// Returns vaht's name for Fluxbox's button `number`.
    fn button(number: u8) -> Option<&'static str> {
        Self::BUTTONS
            .iter()
            .find(|(button, _)| *button == number)
            .map(|(_, name)| *name)
    }

    /// Puts `command` in `document` where `slot` says.
    fn insert(document: &mut DocumentMut, slot: &Slot, command: Value) {
        let table = match (&slot.mode, slot.context) {
            (None, None) => Self::table(document.as_table_mut(), "keys"),
            (Some(mode), _) => {
                let modes = Self::table(document.as_table_mut(), "key_modes");
                modes.set_implicit(true);
                Self::table(modes, mode)
            }
            (None, Some(context)) => {
                let mouse = Self::table(document.as_table_mut(), "mouse");
                mouse.set_implicit(true);
                Self::table(mouse, context)
            }
        };
        table.insert(&slot.trigger, Item::Value(command));
    }

    /// Returns `parent`'s table called `name`, creating it if needed.
    fn table<'table>(parent: &'table mut Table, name: &str) -> &'table mut Table {
        parent
            .entry(name)
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_mut()
            .expect("vaht's tables are tables")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parts of my fluxland `keys` that show every kind of line.
    const KEYS: &str = "\
# click on the desktop to get menus
OnDesktop Mouse4 :PrevWorkspace
OnWindow Mod1 Mouse1 :MacroCmd {Raise} {Focus} {StartMoving}
OnLeftGrip Move1 :StartResizing bottomleft
OnTitlebar Control Mouse1 :StartTabbing
OnTitlebar Double Mouse1 :Maximize
OnTitlebar Move1  :StartMoving
Mod1 Tab :NextWindow {groups} (workspace=[current])
Mod4 Shift Prior :TakeToPrevWorkspace
Control Mod4 F1 :TakeToWorkspace 1
XF86AudioRaiseVolume :Repeat {Exec amixer sset Master 2%+}
Mod4 Tab :NextTab
Mod1 F9 :Minimize
Mod1 F9 :Maximize
";

    #[test]
    fn chains_and_key_modes_convert() {
        let mut report = Report::default();
        let text = "\
Mod4 x Mod4 t :Exec xterm
Mod4 m :KeyMode moving
moving: Left :MoveLeft 10
moving: Mod4 h :Move -10 0
default: Mod4 r :Reconfigure
moving: OnDesktop Mouse1 :RootMenu
Mod4 g Arg :GotoMarkedWindow
None Escape :HideMenus
";
        let document = KeysFile::new(Path::new("keys"), text).convert(&mut report);
        let expected = r#"[keys]
"Super+x Super+t" = "exec xterm"
"Super+m" = "key-mode moving"
"Super+r" = "reload"
Escape = "hide-menus"

[key_modes.moving]
Left = "move -10 0"
"Super+h" = "move -10 0"
"#;
        assert_eq!(document.to_string(), expected);
        let lines: Vec<_> = report
            .notes()
            .iter()
            .map(|note| (note.line, note.reason))
            .collect();
        assert_eq!(
            lines,
            [(6, Reason::NotYetInVaht), (7, Reason::NotYetInVaht)],
            "a mouse binding in a mode, and Arg: {report}"
        );
    }

    #[test]
    fn my_keys_convert() {
        let mut report = Report::default();
        let document = KeysFile::new(Path::new("keys"), KEYS).convert(&mut report);
        let expected = r#"[keys]
"Alt+Tab" = { action = "next-window", windows = { workspace = "current" } }
"Super+Shift+Prior" = "take-to-workspace previous"
"Super+Ctrl+F1" = "take-to-workspace 1"
XF86AudioRaiseVolume = { repeat = "exec amixer sset Master 2%+" }
"Alt+F9" = "maximize"

[mouse.desktop]
ScrollUp = "workspace previous"

[mouse.window]
"Alt+Left" = ["raise", "focus", "start-moving"]

[mouse.grip-left]
"Drag+Left" = "start-resizing bottom-left"

[mouse.titlebar]
"Double+Left" = "maximize"
"Drag+Left" = "start-moving"
"#;
        assert_eq!(document.to_string(), expected);
        let lines: Vec<_> = report
            .notes()
            .iter()
            .map(|note| (note.line, note.reason))
            .collect();
        assert_eq!(
            lines,
            [
                (5, Reason::Dropped),
                (8, Reason::Dropped),
                (12, Reason::Dropped),
                (13, Reason::Dropped),
            ],
            "{report}"
        );
    }
}

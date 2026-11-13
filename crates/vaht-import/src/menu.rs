//! Converting Fluxbox's `menu` and `windowmenu` files to `menus.toml` (02 §8).
//!
//! A Fluxbox menu file is a line per item: `[tag] (label) {command} <icon>`,
//! with `[submenu] (label) {title}` opening a nested menu that `[end]`
//! closes. Each nested menu becomes a `[menu.<name>]` of its own, named
//! from its label, so deep menus stay flat. The root menu is `[menu.root]`
//! and the window menu `[menu.window]`; vaht's workspace menu is written
//! too, as a `menus.toml` replaces every built-in menu. Items vaht does not
//! have are reported, each at its line.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

use crate::pattern::groups;
use crate::report::{Reason, Report};

/// One line of a menu file: its tag, label, command and icon.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MenuLine {
    /// Its line, counting from 1.
    line: usize,
    /// The tag, lowercase: `exec`, `submenu`, `end`…
    tag: String,
    /// The text in parentheses, if any.
    label: Option<String>,
    /// The text in braces, if any.
    command: Option<String>,
    /// The text in angle brackets, if any.
    icon: Option<String>,
}

impl MenuLine {
    /// Reads `text`, the line numbered `line`; none for a blank line or a
    /// comment.
    fn parse(line: usize, text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() || text.starts_with('#') || text.starts_with('!') {
            return None;
        }
        let tag = groups(text, '[', ']').first()?.to_ascii_lowercase();
        let after_tag = &text[text.find(']')? + 1..];
        let first = |open, close| {
            groups(after_tag, open, close)
                .first()
                .map(|group| (*group).to_owned())
        };
        Some(Self {
            line,
            tag,
            label: first('(', ')'),
            command: first('{', '}'),
            icon: first('<', '>'),
        })
    }
}

/// The lines of a menu file being read, and how far.
#[derive(Debug)]
struct Reading<'lines> {
    /// Each line, with the file it is in.
    lines: &'lines [(PathBuf, MenuLine)],
    /// The next line to read.
    position: usize,
}

impl<'lines> Reading<'lines> {
    /// Starts reading `lines` at the first.
    const fn new(lines: &'lines [(PathBuf, MenuLine)]) -> Self {
        Self { lines, position: 0 }
    }

    /// Returns the next line, with its file.
    fn next(&mut self) -> Option<(&'lines Path, &'lines MenuLine)> {
        let (file, line) = self.lines.get(self.position)?;
        self.position += 1;
        Some((file.as_path(), line))
    }
}

/// Fluxbox menu files being converted into one `menus.toml`.
#[derive(Debug)]
pub(crate) struct MenuFiles {
    /// The menus written so far, by name.
    menus: Table,
    /// The names given to menus so far.
    names: HashSet<String>,
}

impl MenuFiles {
    /// How deep `[include]` and `[submenu]` go before the conversion stops
    /// following them: a file that includes itself goes no deeper.
    const MAX_DEPTH: usize = 16;

    /// Starts an empty conversion.
    pub(crate) fn new() -> Self {
        let mut menus = Table::new();
        menus.set_implicit(true);
        Self {
            menus,
            names: HashSet::new(),
        }
    }

    /// Converts the root menu, `text` read from `path`.
    pub(crate) fn convert_root(&mut self, path: &Path, text: &str, report: &mut Report) {
        let lines = Self::lines(path, text, report, 0);
        self.convert_menu("root", None, &mut Reading::new(&lines), report);
    }

    /// Converts the window menu, `text` read from `path`. Without one, vaht's
    /// own is written: Fluxbox's default, less what vaht lacks.
    pub(crate) fn convert_window(&mut self, path: &Path, text: &str, report: &mut Report) {
        let lines = Self::lines(path, text, report, 0);
        self.convert_menu("window", None, &mut Reading::new(&lines), report);
    }

    /// Returns the `menus.toml` written: the menus converted, and vaht's
    /// own window and workspace menus where none was converted, as a
    /// `menus.toml` replaces every built-in menu.
    pub(crate) fn document(mut self) -> DocumentMut {
        let defaults: DocumentMut = vaht_compositor::Config::FILES
            .iter()
            .find(|file| file.name == "menus.toml")
            .expect("menus.toml is one of vaht's files")
            .defaults
            .parse()
            .expect("vaht's built-in menus are valid TOML");
        for name in ["window", "workspace"] {
            if !self.menus.contains_key(name)
                && let Some(built_in) = defaults.get("menu").and_then(|menus| menus.get(name))
            {
                self.menus.insert(name, built_in.clone());
            }
        }
        let mut document = DocumentMut::new();
        document.insert("menu", Item::Table(self.menus));
        document
    }

    /// Returns the lines of `text`, read from `path`, its `[include]`s read
    /// in their place, `depth` includes deep.
    fn lines(
        path: &Path,
        text: &str,
        report: &mut Report,
        depth: usize,
    ) -> Vec<(PathBuf, MenuLine)> {
        let mut lines = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let Some(line) = MenuLine::parse(index + 1, raw) else {
                continue;
            };
            if line.tag != "include" {
                lines.push((path.to_path_buf(), line));
                continue;
            }
            let Some(included) = line.label.as_deref() else {
                continue;
            };
            let included = Self::resolve(path, included);
            match std::fs::read_to_string(&included) {
                Ok(text) if depth < Self::MAX_DEPTH => {
                    lines.extend(Self::lines(&included, &text, report, depth + 1));
                }
                Ok(_) => report.note(
                    path,
                    line.line,
                    Reason::Dropped,
                    "an [include] nested too deep (it includes itself?)",
                ),
                Err(error) => report.note(
                    path,
                    line.line,
                    Reason::NotYetInVaht,
                    format!("[include] {}: {error}", included.display()),
                ),
            }
        }
        lines
    }

    /// Returns `included`, named in the file at `path`: `~/` is the home
    /// directory, and a relative path is taken from the file's directory.
    fn resolve(path: &Path, included: &str) -> PathBuf {
        let expanded = match (included.strip_prefix("~/"), std::env::var_os("HOME")) {
            (Some(rest), Some(home)) => Path::new(&home).join(rest),
            (Some(_) | None, _) => PathBuf::from(included),
        };
        if expanded.is_absolute() {
            expanded
        } else {
            path.parent().unwrap_or(Path::new(".")).join(expanded)
        }
    }

    /// Converts the menu `reading` is at (its `[begin]`, or a `[submenu]`'s
    /// items) to `[menu.<name>]`, titled `title`, up to its `[end]`.
    fn convert_menu(
        &mut self,
        name: &str,
        title: Option<String>,
        reading: &mut Reading<'_>,
        report: &mut Report,
    ) {
        self.names.insert(name.to_owned());
        // Keeps the menu's place ahead of its submenus, which are written
        // as they are met.
        self.menus.insert(name, Item::Table(Table::new()));
        let mut title = title;
        let mut items = Array::new();
        while let Some((file, line)) = reading.next() {
            match line.tag.as_str() {
                "begin" => {
                    if title.is_none() {
                        title.clone_from(&line.label);
                    }
                }
                "end" => break,
                _ => {
                    if line.icon.is_some() {
                        report.note(file, line.line, Reason::NotYetInVaht, "menu icons");
                    }
                    if let Some(item) = self.convert_item(file, line, reading, report) {
                        items.push(item);
                    }
                }
            }
        }
        let mut menu = Table::new();
        if let Some(title) = title {
            menu.insert("title", toml_edit::value(title));
        }
        items.fmt();
        if !items.is_empty() {
            for item in items.iter_mut() {
                item.decor_mut().set_prefix("\n    ");
            }
            items.set_trailing("\n");
            items.set_trailing_comma(true);
        }
        menu.insert("items", toml_edit::value(items));
        self.menus.insert(name, Item::Table(menu));
    }

    /// Returns the item `line` (of `file`) is, reading a submenu's lines
    /// from `reading`; none, with a note, for what vaht lacks.
    fn convert_item(
        &mut self,
        file: &Path,
        line: &MenuLine,
        reading: &mut Reading<'_>,
        report: &mut Report,
    ) -> Option<Value> {
        let label = line.label.clone().or_else(|| {
            // Fluxbox's own labels for its window menu's commands.
            let fluxbox = match line.tag.as_str() {
                "raise" => "Raise",
                "lower" => "Lower",
                "close" => "Close",
                "kill" => "Kill",
                "settitledialog" => "Set Window Title",
                "commanddialog" => "Command",
                _ => return None,
            };
            Some(fluxbox.to_owned())
        });
        let item = |pairs: &[(&str, Value)]| {
            let mut item = InlineTable::new();
            if let Some(label) = &label {
                item.insert("label", Value::from(label.as_str()));
            }
            for (key, value) in pairs {
                item.insert(*key, value.clone());
            }
            Some(Value::InlineTable(item))
        };
        let command = |vaht: &str| item(&[("command", Value::from(vaht))]);
        let mut note = |reason, what: &str| {
            report.note(file, line.line, reason, what);
            None
        };
        match line.tag.as_str() {
            "exec" => {
                if let Some(command_line) = &line.command {
                    command(&format!("exec {command_line}"))
                } else {
                    note(Reason::Dropped, "an [exec] with no command")
                }
            }
            "submenu" => {
                let label = line.label.clone().unwrap_or_else(|| "Menu".to_owned());
                self.convert_submenu(label, line.command.clone(), reading, report)
            }
            "separator" => item(&[("separator", Value::from(true))]),
            "nop" => line.label.as_ref().and_then(|_| item(&[])),
            "workspaces" => item(&[("generate", Value::from("workspaces"))]),
            "reconfig" => command("reload"),
            "exit" => command("exit"),
            "commanddialog" => command("command-dialog"),
            "settitledialog" => command("set-title-dialog"),
            "shade" | "stick" | "maximize" => item(&[("toggle", Value::from(line.tag.as_str()))]),
            "iconify" => item(&[("toggle", Value::from("minimize"))]),
            "raise" | "lower" | "close" | "kill" => command(&line.tag),
            "sendto" => item(&[("generate", Value::from("send-to-workspace"))]),
            "alpha" => item(&[("generate", Value::from("alpha"))]),
            "layer" => item(&[("generate", Value::from("layer"))]),
            "encoding" | "endencoding" => {
                note(Reason::Dropped, "the encoding: menus.toml is UTF-8")
            }
            "config" => note(
                Reason::Dropped,
                "[config]: settings are changed in settings.toml",
            ),
            "extramenus" => note(
                Reason::Dropped,
                "[extramenus]: nothing in vaht adds menus to it",
            ),
            other => note(Reason::NotYetInVaht, &format!("the menu item [{other}]")),
        }
    }

    /// Returns the item opening the submenu labeled `label`, titled
    /// `title` (its label when not given), whose lines `reading` is at;
    /// none if nothing in it converted, as an empty submenu would only be
    /// in the way (each of its items was reported).
    fn convert_submenu(
        &mut self,
        label: String,
        title: Option<String>,
        reading: &mut Reading<'_>,
        report: &mut Report,
    ) -> Option<Value> {
        let name = self.name_for(&label);
        let title = title.or_else(|| Some(label.clone()));
        self.convert_menu(&name, title, reading, report);
        let is_empty = self
            .menus
            .get(&name)
            .and_then(|menu| menu.get("items"))
            .and_then(Item::as_array)
            .is_some_and(Array::is_empty);
        if is_empty {
            self.menus.remove(&name);
            return None;
        }
        let mut item = InlineTable::new();
        item.insert("label", Value::from(label));
        item.insert("submenu", Value::from(name));
        Some(Value::InlineTable(item))
    }

    /// Returns a menu name for a submenu labeled `label`: its words in
    /// lowercase joined by `-`, made unique.
    fn name_for(&self, label: &str) -> String {
        let base: String = label
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join("-");
        let base = if base.is_empty() || ["root", "window", "workspace"].contains(&base.as_str()) {
            format!("{base}-menu").trim_start_matches('-').to_owned()
        } else {
            base
        };
        // Of one more names than are taken, one is free.
        (1..=self.names.len() + 1)
            .map(|number| {
                if number == 1 {
                    base.clone()
                } else {
                    format!("{base}-{number}")
                }
            })
            .find(|name| !self.names.contains(name))
            .expect("some numbered name is free")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root menu as fluxbox-generate_menu writes one.
    const MENU: &str = "\
[begin] (Fluxbox-1.3.7)
[encoding] {UTF-8}
      [exec] (kitty) {kitty}
[submenu] (Misc)
      [exec] (xterm) {xterm} <~/icons/xterm.png>
[end]
[submenu] (Fluxbox menu)
      [config] (Configure)
      [workspaces] (Workspace List)
      [reconfig] (Reload config)
      [restart] (Restart)
      [separator]
      [exit] (Exit)
[end]
[endencoding]
[end]
";

    /// Converts `menu` and the default window menu.
    fn convert(menu: &str) -> (String, Report) {
        let mut report = Report::default();
        let mut files = MenuFiles::new();
        files.convert_root(Path::new("menu"), menu, &mut report);
        (files.document().to_string(), report)
    }

    #[test]
    fn a_menu_file_becomes_flat_named_menus() {
        let (menus, _) = convert(MENU);
        assert!(
            menus.contains("[menu.root]\ntitle = \"Fluxbox-1.3.7\""),
            "{menus}"
        );
        assert!(
            menus.contains("{ label = \"kitty\", command = \"exec kitty\" }"),
            "{menus}"
        );
        assert!(
            menus.contains("{ label = \"Misc\", submenu = \"misc\" }"),
            "{menus}"
        );
        assert!(
            menus.contains("{ label = \"Fluxbox menu\", submenu = \"fluxbox-menu\" }"),
            "{menus}"
        );
        assert!(menus.contains("[menu.misc]\ntitle = \"Misc\""), "{menus}");
        assert!(
            menus.contains("{ label = \"Workspace List\", generate = \"workspaces\" }"),
            "{menus}"
        );
        assert!(menus.contains("{ label = \"Reload config\", command = \"reload\" }"));
        assert!(menus.contains("{ separator = true }"));
        assert!(menus.contains("[menu.workspace]"), "vaht's own: {menus}");
        assert!(
            menus.contains("[menu.window]"),
            "vaht's own, with no windowmenu: {menus}"
        );
    }

    #[test]
    fn what_vaht_lacks_is_reported_at_its_line() {
        let (_, report) = convert(MENU);
        let notes: Vec<(usize, Reason)> = report
            .notes()
            .iter()
            .filter(|note| note.file == Path::new("menu"))
            .map(|note| (note.line, note.reason))
            .collect();
        assert_eq!(
            notes,
            [
                (2, Reason::Dropped),
                (5, Reason::NotYetInVaht),
                (8, Reason::Dropped),
                (11, Reason::NotYetInVaht),
                (15, Reason::Dropped),
            ],
            "encoding, the icon, config, restart, endencoding"
        );
    }

    #[test]
    fn the_window_menu_becomes_toggles_and_commands() {
        let mut report = Report::default();
        let mut files = MenuFiles::new();
        files.convert_window(
            Path::new("windowmenu"),
            "[begin]\n[shade]\n[iconify]\n[settitledialog]\n[sendto]\n[close]\n[end]\n",
            &mut report,
        );
        let menus = files.document().to_string();
        assert!(
            menus.contains("{ toggle = \"minimize\" }"),
            "[iconify] is minimize: {menus}"
        );
        assert!(
            menus.contains("{ generate = \"send-to-workspace\" }"),
            "{menus}"
        );
        assert!(
            menus.contains("{ label = \"Close\", command = \"close\" }"),
            "{menus}"
        );
        assert!(
            menus.contains("{ label = \"Set Window Title\", command = \"set-title-dialog\" }"),
            "{menus}"
        );
        assert!(report.notes().is_empty(), "{report}");
    }

    #[test]
    fn submenu_names_stay_unique() {
        let (menus, _) = convert(
            "[begin]\n[submenu] (Games)\n[exit] (Exit)\n[end]\n[submenu] (Games)\n[exit] (Exit)\n[end]\n\
             [submenu] (Window)\n[exit] (Exit)\n[end]\n[submenu] (Empty)\n[config] (Configure)\n[end]\n[end]\n",
        );
        assert!(menus.contains("submenu = \"games\""), "{menus}");
        assert!(menus.contains("submenu = \"games-2\""), "{menus}");
        assert!(menus.contains("submenu = \"window-menu\""), "{menus}");
        assert!(
            !menus.contains("Empty"),
            "an empty submenu is left out: {menus}"
        );
    }
}

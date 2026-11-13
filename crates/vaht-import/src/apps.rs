//! Converting Fluxbox's `apps` file to `rules.toml` (02 §7).
//!
//! Each `[app] (pattern) ... [end]` block becomes a `[[rule]]`. Fluxbox
//! counts workspaces from 0 in `apps`, vaht from 1. Settings vaht does not
//! have yet are reported, and a block left with nothing to do is dropped.

use std::path::Path;

use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use crate::pattern::{self, ClientPattern};
use crate::report::{Reason, Report};

/// A Fluxbox `apps` file being converted.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AppsFile<'text> {
    /// Where it was read from.
    path: &'text Path,
    /// Its contents.
    text: &'text str,
}

/// One `[Name] (argument) {value}` line of an `[app]` block.
#[derive(Debug, Clone, Copy)]
struct Setting<'text> {
    /// Its line, counting from 1.
    line: usize,
    /// The name, as written.
    name: &'text str,
    /// Everything after the name.
    rest: &'text str,
}

/// One `[app]` block being converted.
#[derive(Debug)]
struct Block {
    /// The line of its `[app]`.
    line: usize,
    /// Its window matcher.
    matcher: InlineTable,
    /// What it does.
    rule: Table,
    /// Whether a pattern could not be converted, so the block would match
    /// windows it should not.
    is_unmatchable: bool,
}

impl<'text> AppsFile<'text> {
    /// Fluxbox's yes-or-no settings (lowercase) and the rule keys vaht
    /// writes them as.
    const FLAGS: [(&str, &str); 3] = [
        ("sticky", "sticky"),
        ("minimized", "minimized"),
        ("pseudotransparent", "pseudo_transparent"),
    ];

    /// Fluxbox's `[Position]` anchors and vaht's.
    const ANCHORS: [(&str, &str); 18] = [
        ("topleft", "top-left"),
        ("upperleft", "top-left"),
        ("top", "top"),
        ("upper", "top"),
        ("topright", "top-right"),
        ("upperright", "top-right"),
        ("left", "left"),
        ("center", "center"),
        ("right", "right"),
        ("bottomleft", "bottom-left"),
        ("lowerleft", "bottom-left"),
        ("bottom", "bottom"),
        ("lower", "bottom"),
        ("bottomright", "bottom-right"),
        ("lowerright", "bottom-right"),
        ("wincenter", "center"),
        ("lefthalf", "left"),
        ("righthalf", "right"),
    ];

    /// An `apps` file read from `path`.
    pub(crate) const fn new(path: &'text Path, text: &'text str) -> Self {
        Self { path, text }
    }

    /// Converts the file, noting in `report` what it leaves out.
    pub(crate) fn convert(self, report: &mut Report) -> DocumentMut {
        let mut rules = ArrayOfTables::new();
        let mut block: Option<Block> = None;
        for (index, line) in self.text.lines().enumerate() {
            let number = index + 1;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            let (keyword, rest) = Self::keyword(line);
            match (keyword.to_ascii_lowercase().as_str(), block.as_mut()) {
                ("app", _) => {
                    block = Some(self.open(number, rest, report));
                }
                ("group", _) => {
                    report.note(
                        self.path,
                        number,
                        Reason::Dropped,
                        "[group] (tabs are gone)",
                    );
                    block = None;
                }
                ("end", Some(_)) => {
                    if let Some(finished) = block.take() {
                        self.close(finished, &mut rules, report);
                    }
                }
                (_, Some(open)) => self.setting(
                    open,
                    Setting {
                        line: number,
                        name: keyword,
                        rest,
                    },
                    report,
                ),
                (_, None) => {}
            }
        }
        let mut document = DocumentMut::new();
        if !rules.is_empty() {
            document.insert("rule", Item::ArrayOfTables(rules));
        }
        document
    }

    /// Splits `[Keyword] rest` into the keyword and the rest.
    fn keyword(line: &str) -> (&str, &str) {
        line.strip_prefix('[')
            .and_then(|inner| inner.split_once(']'))
            .map_or(("", line), |(keyword, rest)| (keyword.trim(), rest.trim()))
    }

    /// Opens a block for `[app] patterns` on line `line`.
    fn open(self, line: usize, patterns: &str, report: &mut Report) -> Block {
        let (matcher, losses) = ClientPattern::new(patterns).matcher();
        let is_unmatchable = !losses.is_empty();
        for (reason, lost) in losses {
            report.note(self.path, line, reason, lost);
        }
        Block {
            line,
            matcher,
            rule: Table::new(),
            is_unmatchable,
        }
    }

    /// Converts `setting` into `block`'s rule, or notes it.
    fn setting(self, block: &mut Block, setting: Setting<'_>, report: &mut Report) {
        let Setting { line, name, rest } = setting;
        let value = pattern::groups(rest, '{', '}')
            .first()
            .copied()
            .unwrap_or_default();
        let argument = pattern::groups(rest, '(', ')')
            .first()
            .copied()
            .unwrap_or_default();
        let is_yes = value.eq_ignore_ascii_case("yes");
        let mut not_yet = || {
            report.note(
                self.path,
                line,
                Reason::NotYetInVaht,
                format!("[{name}] {rest}"),
            );
        };
        match name.to_ascii_lowercase().as_str() {
            "workspace" => match value.parse::<i64>() {
                Ok(zero_based) => {
                    block
                        .rule
                        .insert("workspace", Item::Value(Value::from(zero_based + 1)));
                }
                Err(_) => not_yet(),
            },
            flag if let Some((_, key)) = Self::FLAGS.iter().find(|(name, _)| *name == flag) => {
                block.rule.insert(key, Item::Value(Value::from(is_yes)));
            }
            "fullscreen" if is_yes => {
                block
                    .rule
                    .insert("placement", Item::Value(Value::from("fullscreen")));
            }
            "maximized" => {
                let placement = match value.to_ascii_lowercase().as_str() {
                    "yes" => Some("maximized"),
                    "horz" | "horizontal" => Some("maximized horizontal"),
                    "vert" | "vertical" => Some("maximized vertical"),
                    _ => None,
                };
                if let Some(placement) = placement {
                    block
                        .rule
                        .insert("placement", Item::Value(Value::from(placement)));
                }
            }
            "position" => {
                // Without an anchor, Fluxbox measures from the top left.
                let argument = if argument.is_empty() {
                    "topleft"
                } else {
                    argument
                };
                let anchor = Self::ANCHORS
                    .iter()
                    .find(|(fluxbox, _)| argument.eq_ignore_ascii_case(fluxbox))
                    .map(|(_, vaht)| *vaht);
                match (anchor, Self::pair(value)) {
                    (Some(anchor), Some((x, y))) => {
                        let mut position = InlineTable::new();
                        position.insert("anchor", Value::from(anchor));
                        if (x, y) != (0, 0) {
                            position.insert("offset", Value::Array([x, y].into_iter().collect()));
                        }
                        block
                            .rule
                            .insert("position", Item::Value(Value::InlineTable(position)));
                    }
                    _ => not_yet(),
                }
            }
            "dimensions" => match Self::pair(value) {
                Some((width, height)) if width > 0 && height > 0 => {
                    block.rule.insert(
                        "size",
                        Item::Value(Value::Array([width, height].into_iter().collect())),
                    );
                }
                _ => not_yet(),
            },
            "tab" => report.note(
                self.path,
                line,
                Reason::Dropped,
                format!("[{name}] {rest} (tabs are gone)"),
            ),
            _ => not_yet(),
        }
    }

    /// Finishes `block`: a rule if it matches something and does something.
    fn close(self, block: Block, rules: &mut ArrayOfTables, report: &mut Report) {
        if block.is_unmatchable || block.rule.is_empty() {
            report.note(
                self.path,
                block.line,
                Reason::NotYetInVaht,
                "this [app] block, with nothing vaht can match or do",
            );
            return;
        }
        let mut rule = Table::new();
        rule.insert("match", Item::Value(Value::InlineTable(block.matcher)));
        for (key, item) in block.rule {
            rule.insert(&key, item);
        }
        rules.push(rule);
    }

    /// Reads `x y` as two whole numbers; percentages are not read.
    fn pair(value: &str) -> Option<(i64, i64)> {
        let mut numbers = value.split_whitespace().map(str::parse::<i64>);
        match (numbers.next(), numbers.next(), numbers.next()) {
            (Some(Ok(x)), Some(Ok(y)), None) => Some((x, y)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// My fluxland `apps`, and one block of each other kind.
    const APPS: &str = "\
[app] (class=kitty)
  [PseudoTransparent] {yes}
[end]
[app] (class=fluxland-run)
  [Position] (Center) {0 0}
[end]
[app] (class=steam)
  [FocusProtection] {Gain}
  [Workspace] {1}
[end]
";

    #[test]
    fn my_apps_convert() {
        let mut report = Report::default();
        let document = AppsFile::new(Path::new("apps"), APPS).convert(&mut report);
        assert_eq!(
            document.to_string(),
            "[[rule]]\nmatch = { app_id = \"kitty\" }\npseudo_transparent = true\n\n\
             [[rule]]\nmatch = { app_id = \"vaht-run\" }\nposition = { anchor = \"center\" }\n\n\
             [[rule]]\nmatch = { app_id = \"steam\" }\nworkspace = 2\n"
        );
        let notes: Vec<_> = report
            .notes()
            .iter()
            .map(|note| (note.line, note.reason))
            .collect();
        assert_eq!(notes, [(8, Reason::NotYetInVaht)], "{report}");
    }
}

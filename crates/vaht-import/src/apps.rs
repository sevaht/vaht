//! Converting Fluxbox's `apps` file to `rules.toml` (02 §7).
//!
//! Each `[app] (pattern) ... [end]` block becomes a `[[rule]]`, as does
//! each `[transient]` block. Fluxbox applies `[app]` blocks to windows of
//! their own and `[transient]` blocks to dialogs alone, so each rule's
//! matcher says which (`transient = false` or `true`). Fluxbox
//! counts workspaces from 0 in `apps`, vaht from 1. Settings vaht does not
//! have yet are reported, and a block left with nothing to do is dropped.

use std::path::Path;

use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use crate::command::FluxboxCommand;
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
    const FLAGS: [(&str, &str); 7] = [
        ("sticky", "sticky"),
        ("minimized", "minimized"),
        ("pseudotransparent", "pseudo_transparent"),
        ("shaded", "shaded"),
        ("iconhidden", "hidden_from_window_list"),
        ("focushidden", "hidden_from_cycling"),
        ("ignoresizehints", "ignore_size_hints"),
    ];

    /// Fluxbox's settings (lowercase) whose value is a name vaht writes in
    /// its own words: the rule key, and how the value translates (none for
    /// one vaht lacks).
    #[expect(
        clippy::type_complexity,
        reason = "a table of names, keys and translations reads best inline"
    )]
    const NAMED_VALUES: [(&str, &str, fn(&str) -> Option<&'static str>); 2] = [
        ("layer", "layer", FluxboxCommand::layer),
        ("focusprotection", "focus_stealing", Self::focus_stealing),
    ];

    /// Returns the `position` `[Position] (argument) {value}` gives: an
    /// anchor, and an offset unless it is none; none if vaht cannot say it.
    fn position(argument: &str, value: &str) -> Option<Value> {
        // Without an anchor, Fluxbox measures from the top left.
        let argument = if argument.is_empty() {
            "topleft"
        } else {
            argument
        };
        let anchor = Self::ANCHORS
            .iter()
            .find(|(fluxbox, _)| argument.eq_ignore_ascii_case(fluxbox))
            .map(|(_, vaht)| *vaht)?;
        let (x, y) = Self::pair(value)?;
        let mut position = InlineTable::new();
        position.insert("anchor", Value::from(anchor));
        if (x, y) != (0, 0) {
            position.insert("offset", Value::Array([x, y].into_iter().collect()));
        }
        Some(Value::InlineTable(position))
    }

    /// Returns the `focus_stealing` `[FocusProtection]`'s `value` asks for:
    /// `Gain` lets the window take focus, `Deny` and `Refuse` keep it from
    /// doing so, `None` is the default; none for `Lock`, which vaht lacks.
    fn focus_stealing(value: &str) -> Option<&'static str> {
        let words: Vec<String> = value
            .split([',', ' ', '\t'])
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        let has = |word: &str| words.iter().any(|given| given == word);
        if has("lock") {
            None
        } else if has("gain") {
            Some("allow")
        } else if has("deny") || has("refuse") {
            Some("deny")
        } else if has("none") {
            Some("default")
        } else {
            None
        }
    }

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
                    block = Some(self.open(number, rest, false, report));
                }
                ("transient", _) => {
                    block = Some(self.open(number, rest, true, report));
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

    /// Opens a block for `[app] patterns` on line `line`, or for
    /// `[transient] patterns` if `is_transient`.
    fn open(self, line: usize, patterns: &str, is_transient: bool, report: &mut Report) -> Block {
        let (mut matcher, losses) = ClientPattern::new(patterns).matcher();
        matcher.insert("transient", Value::from(is_transient));
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
        let lowered = name.to_ascii_lowercase();
        if lowered == "tab" {
            report.note(
                self.path,
                line,
                Reason::Dropped,
                format!("[{name}] {rest} (tabs are gone)"),
            );
            return;
        }
        match Self::keys_for(&lowered, argument, value) {
            Some(keys) => {
                for (key, value) in keys {
                    block.rule.insert(key, Item::Value(value));
                }
            }
            None => report.note(
                self.path,
                line,
                Reason::NotYetInVaht,
                format!("[{name}] {rest}"),
            ),
        }
    }

    /// Returns the rule keys the setting `name` (lowercase), with
    /// `argument` and `value`, becomes; none if vaht cannot say it.
    fn keys_for(name: &str, argument: &str, value: &str) -> Option<Vec<(&'static str, Value)>> {
        let is_yes = value.eq_ignore_ascii_case("yes");
        let one = |key: &'static str, value: Value| Some(vec![(key, value)]);
        match name {
            "workspace" => {
                let zero_based = value.trim().parse::<i64>().ok()?;
                one("workspace", Value::from(zero_based + 1))
            }
            flag if let Some((_, key)) = Self::FLAGS.iter().find(|(name, _)| *name == flag) => {
                one(key, Value::from(is_yes))
            }
            "hidden" => Some(vec![
                ("hidden_from_window_list", Value::from(is_yes)),
                ("hidden_from_cycling", Value::from(is_yes)),
            ]),
            "head" => {
                let head = value.trim().parse::<i64>().ok().filter(|head| *head >= 1)?;
                one("output", Value::from(head))
            }
            "deco" => match value.trim().to_ascii_lowercase().as_str() {
                "normal" | "full" => one("frame", Value::from("full")),
                "none" => one("frame", Value::from("none")),
                _ => None,
            },
            "alpha" => one("alpha", Self::alpha(value)?),
            "fullscreen" if is_yes => one("placement", Value::from("fullscreen")),
            "maximized" => {
                let placement = match value.to_ascii_lowercase().as_str() {
                    "yes" => Some("maximized"),
                    "horz" | "horizontal" => Some("maximized horizontal"),
                    "vert" | "vertical" => Some("maximized vertical"),
                    _ => None,
                };
                Some(
                    placement
                        .map(|placement| ("placement", Value::from(placement)))
                        .into_iter()
                        .collect(),
                )
            }
            "position" => one("position", Self::position(argument, value)?),
            named
                if let Some((_, key, translate)) = Self::NAMED_VALUES
                    .iter()
                    .find(|(name, _, _)| *name == named) =>
            {
                one(key, Value::from(translate(value)?))
            }
            "dimensions" => {
                let (width, height) =
                    Self::pair(value).filter(|(width, height)| *width > 0 && *height > 0)?;
                one("size", Value::Array([width, height].into_iter().collect()))
            }
            _ => None,
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

    /// Returns the `alpha` `[Alpha] {focused [unfocused]}` gives, each
    /// clamped to 0 to 255: one number for both, else both.
    fn alpha(value: &str) -> Option<Value> {
        let numbers: Vec<i64> = value
            .split_whitespace()
            .map(|word| word.parse::<i64>().map(|alpha| alpha.clamp(0, 255)))
            .collect::<Result<_, _>>()
            .ok()?;
        match numbers.as_slice() {
            [both] => Some(Value::from(*both)),
            [focused, unfocused] if focused == unfocused => Some(Value::from(*focused)),
            [focused, unfocused] => {
                Some(Value::Array([*focused, *unfocused].into_iter().collect()))
            }
            _ => None,
        }
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

    /// My fluxland `apps`, a `[transient]` block and one of every other
    /// setting.
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
[transient] (title=Save.*)
  [Layer] {AboveDock}
[end]
[app] (class=mpv)
  [Head] {2}
  [Deco] {NONE}
  [Alpha] {255 200}
  [Hidden] {yes}
  [Shaded] {no}
  [IgnoreSizeHints] {yes}
[end]
";

    #[test]
    fn my_apps_convert() {
        let mut report = Report::default();
        let document = AppsFile::new(Path::new("apps"), APPS).convert(&mut report);
        assert_eq!(
            document.to_string(),
            "[[rule]]\nmatch = { app_id = \"kitty\", transient = false }\npseudo_transparent = true\n\n\
             [[rule]]\nmatch = { app_id = \"vaht-run\", transient = false }\n\
             position = { anchor = \"center\" }\n\n\
             [[rule]]\nmatch = { app_id = \"steam\", transient = false }\n\
             focus_stealing = \"allow\"\nworkspace = 2\n\n\
             [[rule]]\nmatch = { title = { regex = \"Save.*\" }, transient = true }\n\
             layer = \"above-dock\"\n\n\
             [[rule]]\nmatch = { app_id = \"mpv\", transient = false }\noutput = 2\n\
             frame = \"none\"\nalpha = [255, 200]\nhidden_from_window_list = true\n\
             hidden_from_cycling = true\nshaded = false\nignore_size_hints = true\n"
        );
        assert!(report.notes().is_empty(), "{report}");
    }
}

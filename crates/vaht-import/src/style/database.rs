//! A Fluxbox style's resources, looked up as Xlib's resource manager does.
//!
//! Style lines are X resources whose names may hold wildcards:
//! `toolbar*.focused.color` and `*.color` both answer a lookup of
//! `toolbar.iconbar.focused.color`. When several match, the more specific
//! wins, decided level by level from the left as `XrmGetResource` decides
//! (and fluxland after it): a name beats `?`, which beats a skipped level,
//! and a tight binding (`.`) beats a loose one (`*`). Names match in any
//! letter case. [`StyleDatabase`] remembers which lines answered a lookup,
//! so the rest can be reported.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One line of a style: a resource name, perhaps with wildcards, and its
/// value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StyleLine {
    /// The name as written.
    pub(crate) name: String,
    /// The value, trimmed.
    pub(crate) value: String,
    /// The line, counting from 1.
    pub(crate) line: usize,
    /// The name's levels, each with whether a `*` binds it loosely.
    levels: Vec<Level>,
}

/// One level of a resource name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Level {
    /// The component: a name, or `?` for any one.
    component: String,
    /// Whether a `*` comes before it, so it may skip levels.
    is_loose: bool,
}

/// The lines of a style.
#[derive(Debug, Clone)]
pub(crate) struct StyleDatabase {
    /// The file they came from.
    path: PathBuf,
    /// Every line, in file order.
    lines: Vec<StyleLine>,
    /// The positions in `lines` of the lines a lookup returned.
    used: BTreeSet<usize>,
}

impl StyleDatabase {
    /// Parses `text`, read from `path`. Comment lines (`!` or `#`), and lines
    /// without a colon or with an unusable name, are skipped.
    pub(crate) fn parse(path: &Path, text: &str) -> Self {
        let lines = text
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                let trimmed = line.trim();
                if trimmed.starts_with('!') || trimmed.starts_with('#') {
                    return None;
                }
                let (name, value) = trimmed.split_once(':')?;
                let name = name.trim();
                Some(StyleLine {
                    name: name.to_owned(),
                    value: value.trim().to_owned(),
                    line: index + 1,
                    levels: Self::levels(name)?,
                })
            })
            .collect();
        Self {
            path: path.to_path_buf(),
            lines,
            used: BTreeSet::new(),
        }
    }

    /// Returns the file the style came from.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the value of the resource `name`, if a line gives it, and
    /// marks that line used. Like Fluxbox, a few window, toolbar and menu
    /// sizes and colors fall back to the style-wide ones (`borderWidth`).
    pub(crate) fn get(&mut self, name: &str) -> Option<&str> {
        let found = self.find(name).or_else(|| {
            let suffix = ["borderWidth", "borderColor", "bevelWidth", "handleWidth"]
                .into_iter()
                .find(|suffix| {
                    ["window.", "toolbar.", "menu.", "slit."]
                        .iter()
                        .any(|prefix| name.strip_prefix(prefix) == Some(suffix))
                })?;
            self.find(suffix)
        })?;
        self.used.insert(found);
        self.lines.get(found).map(|line| line.value.as_str())
    }

    /// Marks the line answering `name` used if it says `value` (in any
    /// letter case): it asks for what vaht does anyway.
    pub(crate) fn accept(&mut self, name: &str, value: &str) {
        let Some(found) = self.find(name) else {
            return;
        };
        if self
            .lines
            .get(found)
            .is_some_and(|line| line.value.trim().eq_ignore_ascii_case(value))
        {
            self.used.insert(found);
        }
    }

    /// Returns the line, counting from 1, answering the resource `name`; 1
    /// if none does.
    pub(crate) fn line_of(&self, name: &str) -> usize {
        self.find(name)
            .and_then(|index| self.lines.get(index))
            .map_or(1, |line| line.line)
    }

    /// Returns the first of `names` a line gives, without marking it used.
    pub(crate) fn first_given<'name>(&self, names: &[&'name str]) -> Option<&'name str> {
        names.iter().copied().find(|name| self.find(name).is_some())
    }

    /// Returns the lines no lookup returned.
    pub(crate) fn unused(&self) -> impl Iterator<Item = &StyleLine> {
        self.lines
            .iter()
            .enumerate()
            .filter(|(index, _)| !self.used.contains(index))
            .map(|(_, line)| line)
    }

    /// Returns the position of the line answering `name`: one naming it
    /// exactly, else the most specific match; the earlier of equals.
    fn find(&self, name: &str) -> Option<usize> {
        if let Some(exact) = self
            .lines
            .iter()
            .rposition(|line| line.name.eq_ignore_ascii_case(name))
        {
            return Some(exact);
        }
        let query: Vec<&str> = name.split('.').collect();
        let mut best: Option<(Vec<u8>, usize)> = None;
        for (index, line) in self.lines.iter().enumerate() {
            let Some(score) = Self::best_match(&line.levels, &query) else {
                continue;
            };
            if best.as_ref().is_none_or(|(known, _)| score > *known) {
                best = Some((score, index));
            }
        }
        best.map(|(_, index)| index)
    }

    /// Returns how well `levels` match `query`, level by level, at their
    /// best alignment: 5 for a name under a tight binding, 4 under a loose
    /// one, 3 and 2 for `?`, and 0 for a level a loose binding skips.
    fn best_match(levels: &[Level], query: &[&str]) -> Option<Vec<u8>> {
        let (Some(level), Some(component)) = (levels.first(), query.first()) else {
            return (levels.is_empty() && query.is_empty()).then(Vec::new);
        };
        let named = if level.component == "?" {
            Some(if level.is_loose { 2 } else { 3 })
        } else if level.component.eq_ignore_ascii_case(component) {
            Some(if level.is_loose { 4 } else { 5 })
        } else {
            None
        };
        let matched = named.and_then(|score| {
            let mut rest = Self::best_match(&levels[1..], &query[1..])?;
            rest.insert(0, score);
            Some(rest)
        });
        let skipped = level
            .is_loose
            .then(|| {
                let mut rest = Self::best_match(levels, &query[1..])?;
                rest.insert(0, 0);
                Some(rest)
            })
            .flatten();
        matched.into_iter().chain(skipped).max()
    }

    /// Splits a resource name into levels; `None` if it ends in a binding
    /// or has no level, which can match nothing.
    fn levels(name: &str) -> Option<Vec<Level>> {
        let mut levels = Vec::new();
        let mut is_loose = false;
        let mut component = String::new();
        for character in name.chars() {
            match character {
                '.' | '*' => {
                    if !component.is_empty() {
                        levels.push(Level {
                            component: std::mem::take(&mut component),
                            is_loose,
                        });
                        is_loose = false;
                    }
                    is_loose |= character == '*';
                }
                other => component.push(other),
            }
        }
        if component.is_empty() {
            return None;
        }
        levels.push(Level {
            component,
            is_loose,
        });
        Some(levels)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// The color lines of my style, with a decoy.
    const STYLE: &str = "\
*.color:\t\t\tgrey30
toolbar*.focused.color:\tgrey55
window*.focus.color:\tgrey55
window.handle.focus.color:\twhite
Menu.Frame.Color:\tblack
*.pressed.color:\tgrey20
";

    #[rstest]
    #[case::wildcard_only("toolbar.clock.color", Some("grey30"))]
    #[case::loose_beats_wildcard("toolbar.iconbar.focused.color", Some("grey55"))]
    #[case::exact_beats_loose("window.handle.focus.color", Some("white"))]
    #[case::loose_under_window("window.label.focus.color", Some("grey55"))]
    #[case::any_case("menu.frame.color", Some("black"))]
    #[case::pressed("window.button.pressed.color", Some("grey20"))]
    #[case::no_match("window.label.focus.textColor", None)]
    fn lookups_follow_the_resource_managers_precedence(
        #[case] name: &str,
        #[case] expected: Option<&str>,
    ) {
        let mut database = StyleDatabase::parse(Path::new("style"), STYLE);
        assert_eq!(database.get(name), expected);
    }

    #[test]
    fn sizes_fall_back_to_the_style_wide_ones() {
        let mut database = StyleDatabase::parse(Path::new("style"), "borderWidth: 3\n");
        assert_eq!(database.get("window.borderWidth"), Some("3"));
        assert_eq!(database.get("window.label.borderWidth"), None);
    }

    #[test]
    fn lines_no_lookup_returned_are_unused() {
        let mut database = StyleDatabase::parse(Path::new("style"), STYLE);
        database.get("toolbar.clock.color");
        let unused: Vec<&str> = database.unused().map(|line| line.name.as_str()).collect();
        assert!(!unused.contains(&"*.color"));
        assert!(unused.contains(&"Menu.Frame.Color"));
    }
}

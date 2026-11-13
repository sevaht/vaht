//! Fluxbox client patterns, and the bracketed groups Fluxbox's syntax
//! writes them and their options in.
//!
//! A [`ClientPattern`] is the `(class=kitty) (workspace=[current])` part
//! of an `apps` block, a `NextWindow` command or the iconbar's mode; it
//! becomes a vaht window matcher (02 §5). [`groups`] finds the top-level
//! `(...)`, `{...}` or `[...]` groups of a line.

use toml_edit::{InlineTable, Value};

use crate::report::Reason;

/// The application id of vaht's run dialog, which takes the place of
/// Fluxbox's `fbrun` and fluxland's `fluxland-run` (a test in the vaht
/// crate keeps it the dialog's).
pub const RUN_DIALOG_APP_ID: &str = "vaht-run";

/// A client pattern: criteria, each in parentheses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClientPattern<'text> {
    /// The text holding the criteria.
    text: &'text str,
}

impl<'text> ClientPattern<'text> {
    /// The pattern of the parenthesized criteria in `text`.
    pub(crate) const fn new(text: &'text str) -> Self {
        Self { text }
    }

    /// Returns the window matcher the criteria make, and each criterion
    /// vaht cannot match yet, with why.
    pub(crate) fn matcher(self) -> (InlineTable, Vec<(Reason, String)>) {
        let mut matcher = InlineTable::new();
        let mut losses = Vec::new();
        for criterion in groups(self.text, '(', ')') {
            if !Self::add_criterion(&mut matcher, criterion) {
                losses.push((Reason::NotYetInVaht, format!("the pattern ({criterion})")));
            }
        }
        (matcher, losses)
    }

    /// Adds `criterion` (`workspace=[current]`, `class!=steam`, or a bare
    /// `workspace`, which means the current one) to `matcher`; returns
    /// whether vaht can match it.
    fn add_criterion(matcher: &mut InlineTable, criterion: &str) -> bool {
        let (key, value) = criterion
            .split_once('=')
            .map_or((criterion, "[current]"), |(key, value)| (key, value.trim()));
        let (key, is_negated) = key
            .strip_suffix('!')
            .map_or((key, false), |key| (key, true));
        // vaht hides no window from its window list, so asking for those it
        // does not hide asks for every window: nothing to add.
        let lowered = key.trim().to_ascii_lowercase();
        if lowered == "iconhidden" && (value.eq_ignore_ascii_case("no") != is_negated) {
            return true;
        }
        let vaht = match (key.trim().to_ascii_lowercase().as_str(), value) {
            ("workspace", "[current]") => Some(("workspace", Value::from("current"))),
            // The run window is vaht's run dialog now.
            ("class", "fbrun" | "fluxland-run") => Some(("app_id", Value::from(RUN_DIALOG_APP_ID))),
            ("class", text) => Some(("app_id", Self::text_pattern(text))),
            ("title", text) => Some(("title", Self::text_pattern(text))),
            ("minimized", "yes") => Some(("minimized", Value::from(true))),
            ("minimized", "no") => Some(("minimized", Value::from(false))),
            _ => None,
        };
        let Some((key, value)) = vaht else {
            return false;
        };
        if is_negated {
            let mut excluded = InlineTable::new();
            excluded.insert(key, value);
            matcher.insert("not", Value::InlineTable(excluded));
        } else {
            matcher.insert(key, value);
        }
        true
    }

    /// Returns a pattern's value as a vaht text pattern: exact if it has
    /// no regular expression syntax, otherwise `{ regex = .. }`, since
    /// Fluxbox patterns are regular expressions.
    fn text_pattern(text: &str) -> Value {
        let is_plain = text
            .chars()
            .all(|character| character.is_alphanumeric() || "-_ ".contains(character));
        if is_plain {
            Value::from(text)
        } else {
            let mut regex = InlineTable::new();
            regex.insert("regex", Value::from(text));
            Value::InlineTable(regex)
        }
    }
}

/// Returns the contents of the top-level groups `open`...`close` in
/// `text`, nested groups included in their parent's contents.
pub(crate) fn groups(text: &str, open: char, close: char) -> Vec<&str> {
    let mut groups = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0;
    for (index, character) in text.char_indices() {
        if character == open {
            if depth == 0 {
                start = index + character.len_utf8();
            }
            depth += 1;
        } else if character == close && depth > 0 {
            depth -= 1;
            if depth == 0 {
                groups.push(text[start..index].trim());
            }
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::iconbar_mode("{static groups} (workspace)", "{ workspace = \"current\" }")]
    #[case::class_and_negated_class(
        "(workspace=[current]) (class!=steam.*)",
        "{ workspace = \"current\", not = { app_id = { regex = \"steam.*\" } } }"
    )]
    #[case::exact_class("(class=kitty)", "{ app_id = \"kitty\" }")]
    fn patterns_become_matchers(#[case] text: &str, #[case] expected: &str) {
        let (matcher, losses) = ClientPattern::new(text).matcher();
        assert_eq!(losses, []);
        assert_eq!(matcher.to_string().trim(), expected);
    }

    #[test]
    fn criteria_vaht_cannot_match_are_lost() {
        let (_, losses) = ClientPattern::new("(layer=dock) (title=x)").matcher();
        assert_eq!(
            losses,
            [(Reason::NotYetInVaht, "the pattern (layer=dock)".to_owned())]
        );
    }

    #[test]
    fn groups_nest() {
        assert_eq!(groups("{a {b}} {c}", '{', '}'), ["a {b}", "c"]);
    }
}

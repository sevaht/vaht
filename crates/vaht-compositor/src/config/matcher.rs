//! Window matchers: which windows a command or rule is about (02 §5).
//!
//! ```toml
//! { app_id = "kitty" }
//! { title = { regex = "^Steam$" }, workspace = "current" }
//! { minimized = "any", not = { app_id = "steam" } }
//! ```
//!
//! Every key given must match. Plain strings match exactly; `{ regex = ".." }`
//! is an anchored regular expression, compiled once when the configuration
//! is read. Fluxbox patterns were always regular expressions, which made
//! `firefox.desktop` match `firefox_desktop`.

use std::fmt;
use std::num::NonZeroUsize;

use regex::Regex;
use serde::de::{self, Deserializer, MapAccess, Visitor};

use crate::window_manager::{Window, Workspace};

/// Which windows something is about.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WindowMatcher {
    /// The Wayland app id (or X11 class).
    app_id: Option<TextPattern>,
    /// The title.
    title: Option<TextPattern>,
    /// The workspace it is on.
    workspace: Option<WorkspaceSelector>,
    /// Whether it is minimized.
    minimized: Option<StateSelector>,
    /// Whether it is on every workspace.
    sticky: Option<StateSelector>,
    /// Whether it is a dialog of another window (Fluxbox's `[transient]`).
    transient: Option<StateSelector>,
    /// What it must not match.
    not: Option<Box<WindowMatcher>>,
}

impl WindowMatcher {
    /// Returns the matcher of the windows on the workspace shown (sticky
    /// ones included).
    pub(crate) fn on_current_workspace() -> Self {
        Self {
            workspace: Some(WorkspaceSelector::Current),
            ..Self::default()
        }
    }

    /// Returns whether `window` matches while `current` is the workspace
    /// shown.
    pub(crate) fn matches(&self, window: &Window, current: Workspace) -> bool {
        let text_matches = |pattern: &Option<TextPattern>, text: &str| {
            pattern.as_ref().is_none_or(|pattern| pattern.matches(text))
        };
        let state = window.state();
        text_matches(&self.app_id, window.app_id())
            && text_matches(&self.title, window.title())
            && self.workspace.is_none_or(|selector| {
                state.is_sticky || selector.workspace(current) == window.workspace()
            })
            && self
                .minimized
                .is_none_or(|selector| selector.matches(state.is_minimized))
            && self
                .sticky
                .is_none_or(|selector| selector.matches(state.is_sticky))
            && self
                .transient
                .is_none_or(|selector| selector.matches(window.parent().is_some()))
            && self
                .not
                .as_ref()
                .is_none_or(|excluded| !excluded.matches(window, current))
    }

    /// Returns whether minimized windows can match: only when the matcher
    /// asks for them, since window cycling skips them otherwise.
    pub(crate) fn admits_minimized(&self) -> bool {
        matches!(
            self.minimized,
            Some(StateSelector::Yes | StateSelector::Any)
        )
    }
}

/// Text to match: exactly, or by an anchored regular expression.
#[derive(Debug, Clone)]
enum TextPattern {
    /// Exactly this text.
    Exact(String),
    /// Text this expression matches as a whole.
    Regex(Regex),
}

impl TextPattern {
    /// Returns whether `text` matches.
    fn matches(&self, text: &str) -> bool {
        match self {
            Self::Exact(exact) => exact == text,
            Self::Regex(regex) => regex.is_match(text),
        }
    }

    /// Compiles `pattern` to match whole texts only.
    fn regex(pattern: &str) -> Result<Self, regex::Error> {
        Regex::new(&format!("^(?:{pattern})$")).map(Self::Regex)
    }
}

impl PartialEq for TextPattern {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(text), Self::Exact(other)) => text == other,
            (Self::Regex(regex), Self::Regex(other)) => regex.as_str() == other.as_str(),
            (Self::Exact(_), Self::Regex(_)) | (Self::Regex(_), Self::Exact(_)) => false,
        }
    }
}

impl Eq for TextPattern {}

impl<'de> serde::Deserialize<'de> for TextPattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads a string or a `{ regex = ".." }` table.
        struct PatternVisitor;

        impl<'de> Visitor<'de> for PatternVisitor {
            type Value = TextPattern;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string, or { regex = \"...\" }")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<TextPattern, E> {
                Ok(TextPattern::Exact(text.to_owned()))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<TextPattern, A::Error> {
                let (Some((key, pattern)), None) = (
                    map.next_entry::<String, String>()?,
                    map.next_key::<String>()?,
                ) else {
                    return Err(de::Error::custom("a pattern table holds only `regex`"));
                };
                if key != "regex" {
                    return Err(de::Error::custom(format!(
                        "a pattern table holds `regex`, not `{key}`"
                    )));
                }
                TextPattern::regex(&pattern).map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_any(PatternVisitor)
    }
}

/// Which workspace a matcher means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceSelector {
    /// The one shown.
    Current,
    /// The one with this 1-based number.
    Numbered(NonZeroUsize),
}

impl WorkspaceSelector {
    /// Returns the workspace meant while `current` is shown.
    const fn workspace(self, current: Workspace) -> Workspace {
        match self {
            Self::Current => current,
            Self::Numbered(number) => Workspace::numbered(number),
        }
    }
}

impl<'de> serde::Deserialize<'de> for WorkspaceSelector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads `"current"` or a number.
        struct SelectorVisitor;

        impl Visitor<'_> for SelectorVisitor {
            type Value = WorkspaceSelector;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("\"current\" or a workspace number from 1")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<WorkspaceSelector, E> {
                if text == "current" {
                    Ok(WorkspaceSelector::Current)
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(text), &self))
                }
            }

            fn visit_i64<E: de::Error>(self, number: i64) -> Result<WorkspaceSelector, E> {
                usize::try_from(number)
                    .ok()
                    .and_then(NonZeroUsize::new)
                    .map(WorkspaceSelector::Numbered)
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Signed(number), &self))
            }
        }

        deserializer.deserialize_any(SelectorVisitor)
    }
}

/// Whether a window must be in a state: yes, no, or either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateSelector {
    /// It must be.
    Yes,
    /// It must not be.
    No,
    /// Either.
    Any,
}

impl StateSelector {
    /// Returns whether a window whose state `is_in_state` matches.
    const fn matches(self, is_in_state: bool) -> bool {
        match self {
            Self::Yes => is_in_state,
            Self::No => !is_in_state,
            Self::Any => true,
        }
    }
}

impl<'de> serde::Deserialize<'de> for StateSelector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads `true`, `false` or `"any"`.
        struct StateVisitor;

        impl Visitor<'_> for StateVisitor {
            type Value = StateSelector;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("true, false or \"any\"")
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<StateSelector, E> {
                Ok(if value {
                    StateSelector::Yes
                } else {
                    StateSelector::No
                })
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<StateSelector, E> {
                if text == "any" {
                    Ok(StateSelector::Any)
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(text), &self))
                }
            }
        }

        deserializer.deserialize_any(StateVisitor)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Parses a matcher written as a TOML inline table.
    fn matcher(text: &str) -> Result<WindowMatcher, String> {
        #[derive(serde::Deserialize)]
        struct Holder {
            /// The matcher.
            windows: WindowMatcher,
        }
        toml::from_str::<Holder>(&format!("windows = {text}"))
            .map(|holder| holder.windows)
            .map_err(|error| error.message().to_owned())
    }

    #[rstest]
    #[case::exact("kitty", true)]
    #[case::prefix_only("kitty-tools", false)]
    fn exact_text_matches_whole(#[case] text: &str, #[case] expected: bool) {
        assert_eq!(
            TextPattern::Exact("kitty".to_owned()).matches(text),
            expected
        );
    }

    #[rstest]
    #[case::whole("Steam", true)]
    #[case::part_of_more("Steam - News", false)]
    fn a_regex_is_anchored(#[case] text: &str, #[case] expected: bool) {
        let pattern = TextPattern::regex("Steam").expect("a valid expression");
        assert_eq!(pattern.matches(text), expected);
    }

    #[test]
    fn the_cycling_matcher_of_my_setup_reads() {
        let parsed = matcher("{ workspace = \"current\", minimized = \"any\", not = { app_id = { regex = \"steam.*\" } } }")
            .expect("valid");
        assert!(parsed.admits_minimized());
        assert!(!WindowMatcher::default().admits_minimized());
    }

    #[rstest]
    #[case::unknown_key("{ class = \"kitty\" }", "unknown field")]
    #[case::bad_regex("{ title = { regex = \"(\" } }", "regex parse error")]
    #[case::workspace_zero("{ workspace = 0 }", "workspace number from 1")]
    #[case::bad_state("{ minimized = \"maybe\" }", "true, false or")]
    fn mistakes_are_explained(#[case] text: &str, #[case] expected: &str) {
        let error = matcher(text).expect_err(text);
        assert!(error.contains(expected), "{error}");
    }
}

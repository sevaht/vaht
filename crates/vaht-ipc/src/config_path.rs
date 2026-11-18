//! Where a setting is in a configuration file: its parts from the top,
//! written `focus.model`, or `input.device."My Mouse".accel_speed` when a
//! part is not a plain word.
//!
//! One grammar for everything that names a setting: the `set-setting`
//! action, `vaht config set`, and the edits the settings window sends.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The address of a setting: the keys to follow from the top of its file.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ConfigPath(Vec<String>);

/// Why text is not a [`ConfigPath`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigPathError {
    /// No key where one is needed.
    #[error("a key is needed, such as focus.model")]
    Missing,
    /// A quoted part is not closed.
    #[error("the key `{0}` has an unclosed quote")]
    UnclosedQuote(String),
    /// The text does not begin with a key.
    #[error("`{0}` does not begin with a key")]
    NotAKey(String),
}

impl ConfigPath {
    /// Returns the path through `parts`; none if there are none.
    #[must_use]
    pub fn from_parts(parts: Vec<String>) -> Option<Self> {
        (!parts.is_empty()).then_some(Self(parts))
    }

    /// Returns the keys to follow, from the top of the file.
    #[must_use]
    pub fn parts(&self) -> &[String] {
        &self.0
    }

    /// Reads the path `text` begins with, and returns it with what follows
    /// (trimmed). A part is a plain word (letters, digits, `_`, `-`) or
    /// quoted; parts are separated by `.`; the path ends at whitespace or
    /// the end.
    ///
    /// # Errors
    ///
    /// Returns what is wrong with the key.
    pub fn parse_leading(text: &str) -> Result<(Self, &str), ConfigPathError> {
        let mut parts = Vec::new();
        let mut rest = text.trim_start();
        loop {
            let (part, after) = if let Some(quoted) = rest.strip_prefix('"') {
                let end = quoted
                    .find('"')
                    .ok_or_else(|| ConfigPathError::UnclosedQuote(text.to_owned()))?;
                (&quoted[..end], &quoted[end + 1..])
            } else {
                let end = rest
                    .find(|character: char| !Self::is_plain(character))
                    .unwrap_or(rest.len());
                (&rest[..end], &rest[end..])
            };
            if part.is_empty() {
                return Err(ConfigPathError::Missing);
            }
            parts.push(part.to_owned());
            match after.strip_prefix('.') {
                Some(next) => rest = next,
                None if after.is_empty() || after.starts_with(char::is_whitespace) => {
                    return Ok((Self(parts), after.trim()));
                }
                None => return Err(ConfigPathError::NotAKey(text.to_owned())),
            }
        }
    }

    /// Reads `text`, all of it a path.
    ///
    /// # Errors
    ///
    /// Returns what is wrong with the key, or that more follows it.
    pub fn parse(text: &str) -> Result<Self, ConfigPathError> {
        let (path, rest) = Self::parse_leading(text)?;
        if rest.is_empty() {
            Ok(path)
        } else {
            Err(ConfigPathError::NotAKey(text.to_owned()))
        }
    }

    /// Returns whether `character` may be in a part written without quotes.
    const fn is_plain(character: char) -> bool {
        character.is_ascii_alphanumeric() || character == '_' || character == '-'
    }
}

impl fmt::Display for ConfigPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, part) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(".")?;
            }
            if !part.is_empty() && part.chars().all(Self::is_plain) {
                formatter.write_str(part)?;
            } else {
                write!(formatter, "\"{part}\"")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::one_part("focus", &["focus"], "")]
    #[case::dotted("focus.model sloppy", &["focus", "model"], "sloppy")]
    #[case::quoted(r#"input.device."My Mouse".accel_speed 0.5"#, &["input", "device", "My Mouse", "accel_speed"], "0.5")]
    #[case::a_dot_in_quotes(r#"a."b.c""#, &["a", "b.c"], "")]
    fn a_path_is_read_up_to_whitespace(
        #[case] text: &str,
        #[case] parts: &[&str],
        #[case] rest: &str,
    ) {
        let (path, after) = ConfigPath::parse_leading(text).expect("a path");
        assert_eq!(path.parts(), parts);
        assert_eq!(after, rest);
    }

    #[rstest]
    #[case::empty("")]
    #[case::leading_dot(".focus")]
    #[case::trailing_dot("focus.")]
    #[case::unclosed_quote(r#"a."b"#)]
    #[case::not_a_key("focus=1")]
    fn text_that_is_not_a_path_is_refused(#[case] text: &str) {
        assert!(ConfigPath::parse_leading(text).is_err(), "{text}");
    }

    #[rstest]
    #[case::plain("focus.model")]
    #[case::quoted(r#"input.device."My Mouse".accel_speed"#)]
    fn a_path_displays_as_it_parses(#[case] text: &str) {
        assert_eq!(ConfigPath::parse(text).expect("a path").to_string(), text);
    }

    #[test]
    fn a_path_is_its_parts_in_json() {
        let path = ConfigPath::parse("blur.spread").expect("a path");
        assert_eq!(
            serde_json::to_string(&path).expect("encodes"),
            r#"["blur","spread"]"#
        );
    }
}

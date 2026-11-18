//! Themes: `themes/<name>.toml`, a style and a layout under one name (02 §10).
//!
//! A theme says only which style and which layout go together:
//!
//! ```toml
//! style = "nord"
//! layout = "slim"
//! ```
//!
//! `settings.toml`'s `theme` picks one. Its own `style` and `layout`, when it
//! has them, win over the theme's, so a theme can be taken with a different
//! style or layout. Both parts are optional: a theme may name only one.

use std::path::{Path, PathBuf};

use super::ConfigError;
use super::library::Library;

/// What a theme file says.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ThemeParts {
    /// The style the theme uses, if it names one.
    pub(crate) style: Option<String>,
    /// The layout the theme uses, if it names one.
    pub(crate) layout: Option<String>,
}

impl ThemeParts {
    /// Puts the theme's style and layout into `settings`, a table of
    /// `settings.toml`'s, for each that `settings` does not name itself.
    pub(crate) fn lay_under(&self, settings: &mut toml::Table) {
        for (key, part) in [("style", &self.style), ("layout", &self.layout)] {
            if let Some(name) = part
                && !settings.contains_key(key)
            {
                settings.insert(key.to_owned(), toml::Value::String(name.clone()));
            }
        }
    }
}

/// A theme file, found by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeFile {
    /// Where it is.
    path: PathBuf,
}

impl ThemeFile {
    /// Finds the theme `name`: in the `themes` directory of the configuration
    /// `directory`, then among the installed themes.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` is not a plain name, or no such theme
    /// exists.
    pub(crate) fn find(directory: &Path, name: &str) -> Result<Self, ConfigError> {
        Library::THEMES
            .find(directory, name)
            .map(|path| Self { path })
    }

    /// Returns the names of the themes there are, sorted, each once.
    pub(crate) fn available(directory: &Path) -> Vec<String> {
        Library::THEMES.available(directory)
    }

    /// Reads the theme.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, is not TOML, or says
    /// anything but a style and a layout, with the line and column.
    pub(crate) fn read(&self) -> Result<ThemeParts, ConfigError> {
        let text = std::fs::read_to_string(&self.path).map_err(|error| ConfigError::Read {
            path: self.path.clone(),
            error,
        })?;
        toml::from_str(&text)
            .map_err(|error| ConfigError::invalid(self.path.clone(), &text, &error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_fills_in_only_what_settings_leave_out() {
        let theme = ThemeParts {
            style: Some("nord".to_owned()),
            layout: Some("slim".to_owned()),
        };
        let mut settings: toml::Table = toml::from_str("layout = \"full\"").expect("TOML");
        theme.lay_under(&mut settings);
        assert_eq!(settings["style"].as_str(), Some("nord"));
        assert_eq!(
            settings["layout"].as_str(),
            Some("full"),
            "the file's own wins"
        );
    }

    #[test]
    fn a_theme_may_name_one_part_and_nothing_else() {
        let one: ThemeParts = toml::from_str("style = \"nord\"").expect("valid");
        assert_eq!(one.layout, None);
        assert!(toml::from_str::<ThemeParts>("colour = 1").is_err());
    }
}

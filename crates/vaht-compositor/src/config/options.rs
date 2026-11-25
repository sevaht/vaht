//! The library of choices shared by every style: `options/<option>/<choice>.toml`
//! (02 §10.5).
//!
//! A choice file is a table of variables. It is found in the user's
//! configuration directory, then in the system's; a style that declares the
//! option and uses the same variable names takes it up.

use std::path::{Path, PathBuf};

use super::library::system_directory;
use vaht_ui::Style;
use vaht_ui::style::options::ChoiceLibrary;

/// The directory of choice directories in the configuration directory.
pub(crate) const DIRECTORY: &str = "options";

/// An option of the style in use with everything there is to choose from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListedOption {
    /// Its name.
    pub(crate) name: String,
    /// What to call it for people, if the style says.
    pub(crate) label: Option<String>,
    /// The choice made when none is.
    pub(crate) default: String,
    /// The choice in effect.
    pub(crate) chosen: String,
    /// The choices: the style's own and the shared ones, the default first,
    /// the rest sorted.
    pub(crate) choices: Vec<String>,
}

impl ListedOption {
    /// Returns what to call the option: its label, else its name.
    pub(crate) fn title(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

/// The shared choices, as the configuration `directory` and the system have
/// them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OptionLibrary<'directory> {
    /// The configuration directory.
    directory: &'directory Path,
}

impl<'directory> OptionLibrary<'directory> {
    /// The library of the configuration `directory`, and the system's.
    pub(crate) const fn new(directory: &'directory Path) -> Self {
        Self { directory }
    }

    /// Returns whether `name` is a plain name for an option or a choice.
    fn is_plain(name: &str) -> bool {
        !name.is_empty() && !name.contains(['/', '\0']) && !name.starts_with('.')
    }

    /// Returns the directories `option`'s choices are in: the user's first.
    fn directories(&self, option: &str) -> [PathBuf; 2] {
        [
            self.directory.join(DIRECTORY).join(option),
            system_directory().join(DIRECTORY).join(option),
        ]
    }

    /// Returns the options `style` declares, each with every choice there is
    /// for it: the style's own and the library's.
    pub(crate) fn listed(&self, style: &Style) -> Vec<ListedOption> {
        style
            .options()
            .options
            .iter()
            .map(|option| {
                let mut choices: Vec<String> = option
                    .choices
                    .iter()
                    .cloned()
                    .chain(self.choices(&option.name))
                    .collect();
                choices.sort_by_key(|choice| (*choice != option.default, choice.clone()));
                choices.dedup();
                ListedOption {
                    name: option.name.clone(),
                    label: option.label.clone(),
                    default: option.default.clone(),
                    chosen: option.chosen.clone(),
                    choices,
                }
            })
            .collect()
    }

    /// Returns the names of the choices of `option` there are, sorted, each
    /// once.
    pub(crate) fn choices(&self, option: &str) -> Vec<String> {
        if !Self::is_plain(option) {
            return Vec::new();
        }
        let mut names: Vec<String> = self
            .directories(option)
            .iter()
            .filter_map(|directory| std::fs::read_dir(directory).ok())
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let is_choice = path
                    .extension()
                    .is_some_and(|extension| extension == "toml");
                is_choice.then(|| path.file_stem()?.to_str().map(ToOwned::to_owned))?
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

impl ChoiceLibrary for OptionLibrary<'_> {
    fn variables(&self, option: &str, choice: &str) -> Result<Option<toml::Table>, String> {
        if !Self::is_plain(option) || !Self::is_plain(choice) {
            return Ok(None);
        }
        let file_name = format!("{choice}.toml");
        let Some(path) = self
            .directories(option)
            .into_iter()
            .map(|directory| directory.join(&file_name))
            .find(|path| path.is_file())
        else {
            return Ok(None);
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        toml::from_str(&text)
            .map(Some)
            .map_err(|error| format!("{}: {}", path.display(), error.message()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vaht_ui::text::TextEffect;
    use vaht_ui::{StyleElement, WidgetState};

    fn configuration(files: &[(&str, &str)]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("a temporary directory");
        for (name, text) in files {
            let path = directory.path().join(name);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("directories");
            std::fs::write(path, text).expect("written");
        }
        directory
    }

    #[test]
    fn a_choice_file_is_read_and_listed() {
        let directory = configuration(&[
            ("options/shade/dusk.toml", "bar = \"navy\"\n"),
            ("options/shade/notes.txt", ""),
        ]);
        let library = OptionLibrary::new(directory.path());
        assert!(library.choices("shade").contains(&"dusk".to_owned()));
        assert!(!library.choices("shade").contains(&"notes".to_owned()));
        let table = library
            .variables("shade", "dusk")
            .expect("readable")
            .expect("there");
        assert_eq!(table["bar"].as_str(), Some("navy"));
        assert_eq!(library.variables("shade", "nowhere"), Ok(None));
    }

    #[test]
    fn a_choice_that_is_not_toml_is_an_error_naming_the_file() {
        let directory = configuration(&[("options/shade/bad.toml", "bar = \n")]);
        let error = OptionLibrary::new(directory.path())
            .variables("shade", "bad")
            .expect_err("not TOML");
        assert!(error.contains("bad.toml"), "{error}");
    }

    #[test]
    fn names_that_are_paths_find_nothing() {
        let directory = configuration(&[("options/shade/dusk.toml", "")]);
        let library = OptionLibrary::new(directory.path());
        assert_eq!(library.variables("../options", "dusk"), Ok(None));
        assert_eq!(library.variables("shade", "../shade/dusk"), Ok(None));
        assert_eq!(library.choices(".."), Vec::<String>::new());
    }

    /// The choices shipped in `data/options`, as a library.
    struct Shipped;

    impl ChoiceLibrary for Shipped {
        fn variables(&self, option: &str, choice: &str) -> Result<Option<toml::Table>, String> {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../data/options")
                .join(option)
                .join(format!("{choice}.toml"));
            match std::fs::read_to_string(&path) {
                Ok(text) => toml::from_str(&text)
                    .map(Some)
                    .map_err(|error| format!("{}: {}", path.display(), error.message())),
                Err(_) => Ok(None),
            }
        }
    }

    /// The default style with `chosen` made, from the shipped choices.
    fn load(chosen: &[(&str, &str)]) -> Style {
        use std::collections::BTreeMap;

        use vaht_ui::style::options::Selection;

        let chosen: BTreeMap<String, String> = chosen
            .iter()
            .map(|(option, choice)| ((*option).to_owned(), (*choice).to_owned()))
            .collect();
        Style::built_in_selecting(&Selection {
            directory: None,
            chosen: &chosen,
            library: &Shipped,
        })
    }

    /// The shipped schemes: their names and files.
    fn shipped_schemes() -> Vec<(String, PathBuf)> {
        std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/options/scheme"))
            .expect("the shipped schemes")
            .map(|entry| {
                let path = entry.expect("an entry").path();
                let name = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .expect("a name")
                    .to_owned();
                (name, path)
            })
            .collect()
    }

    #[test]
    fn every_shipped_scheme_loads_with_the_default_style_and_looks_different() {
        let default = load(&[]);
        assert_eq!(default.option_problems(), Vec::<String>::new());
        let vocabulary: toml::Table = toml::from_str(Style::BUILT_IN).expect("TOML");
        let vocabulary = vocabulary["options"]["scheme"]["graphite"]
            .as_table()
            .expect("the default scheme");
        let schemes = shipped_schemes();
        assert!(
            schemes.len() >= 18,
            "the shipped schemes: {}",
            schemes.len()
        );
        for (name, path) in schemes {
            let table: toml::Table =
                toml::from_str(&std::fs::read_to_string(&path).expect("readable")).expect("TOML");
            let style = load(&[("scheme", &name)]);
            assert_eq!(style.option_problems(), Vec::<String>::new(), "{name}");
            let chosen = &style
                .options()
                .options
                .iter()
                .find(|option| option.name == "scheme")
                .expect("the default style has a scheme option")
                .chosen;
            assert_eq!(chosen, &name);
            assert_ne!(style, default, "{name} changes the look");
            // It sets only variables the default style defines.
            for key in table.keys() {
                assert!(
                    vocabulary.contains_key(key),
                    "{name} sets `{key}`, which the default style has no variable for"
                );
            }
        }
    }

    #[test]
    fn every_other_choice_of_the_default_style_loads_and_looks_different() {
        let default = load(&[]);
        for (option, choice) in [
            ("titlebar_size", "small"),
            ("titlebar_size", "large"),
            ("taskbar_size", "small"),
            ("taskbar_size", "xlarge"),
            ("menu_size", "small"),
            ("menu_size", "large"),
            ("font", "liberation_sans"),
            ("menu_title", "slate"),
            ("menu_marks", "chevrons"),
            ("text_outline", "black"),
            ("text_outline", "none"),
            ("buttons", "flat"),
        ] {
            let style = load(&[(option, choice)]);
            assert_eq!(
                style.option_problems(),
                Vec::<String>::new(),
                "{option} {choice}"
            );
            assert_ne!(style, default, "{option} {choice} changes the look");
        }
    }

    #[test]
    fn the_text_outline_and_buttons_choices_change_what_they_name() {
        let looks = |style: &Style| {
            (
                style
                    .element_style(StyleElement::Label, WidgetState::Focused)
                    .text
                    .effect,
                style
                    .element_style(StyleElement::Button, WidgetState::Focused)
                    .texture
                    .clone(),
            )
        };
        let (effect, button) = looks(&load(&[]));
        assert_ne!(effect, TextEffect::None, "graphite has an outline");
        let (flat_effect, flat_button) = looks(&load(&[("buttons", "flat")]));
        assert_eq!(flat_effect, effect, "buttons leave the text alone");
        assert_ne!(flat_button, button, "flat buttons have no background");
        let (none_effect, none_button) = looks(&load(&[("text_outline", "none")]));
        assert_eq!(none_effect, TextEffect::None);
        assert_eq!(none_button, button, "the outline leaves buttons alone");
        let (nord_effect, _) = looks(&load(&[("scheme", "nord")]));
        assert_eq!(
            nord_effect,
            TextEffect::None,
            "nord has no outline of its own"
        );
        let (nord_outlined, _) = looks(&load(&[("scheme", "nord"), ("text_outline", "black")]));
        assert_ne!(nord_outlined, TextEffect::None, "but one can be asked for");
    }

    #[test]
    fn every_choice_of_every_option_works_with_every_scheme() {
        let default = load(&[]);
        for option in &default.options().options {
            for choice in &option.choices {
                for (scheme, _) in shipped_schemes() {
                    let style = load(&[(option.name.as_str(), choice), ("scheme", &scheme)]);
                    assert_eq!(
                        style.option_problems(),
                        Vec::<String>::new(),
                        "{} = {choice} with {scheme}",
                        option.name
                    );
                }
            }
        }
    }
}

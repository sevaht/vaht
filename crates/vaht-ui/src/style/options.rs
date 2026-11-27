//! Options of a style (02 §10.5): named sets of values a style lets people
//! choose between, such as a color scheme or a size.
//!
//! A style declares each option under `[options.<option>]`: a `default`
//! choice, which is written inline, and any other choices of its own, each
//! inline or `file = "name.toml"` beside the style. A choice is a table of
//! **variables**, and the style's properties use them: a value that is
//! `"$name"` is the variable's value (a color, a texture, a length, anything),
//! and `$name` inside text is the variable's text.
//!
//! The variables a choice gives, lowest to highest: the default choice's; the
//! style's own choice of that name; a choice file of that name in the library
//! of every style's choices (`options/<option>/<choice>.toml`, in the user's
//! configuration directory or the system's). Only variables the default
//! choice defines are taken, so a choice that leaves some out keeps the
//! default's, and a choice for one style works for any style that has the
//! same option and variable names.

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path};

use serde::de::Error as _;

/// Where choices that are not a style's own are found: the library of
/// choices shared by every style.
pub trait ChoiceLibrary {
    /// Returns the variables of `choice` of `option`, none if the library has
    /// no such choice.
    ///
    /// # Errors
    ///
    /// Returns why the library has one that cannot be read.
    fn variables(&self, option: &str, choice: &str) -> Result<Option<toml::Table>, String>;
}

/// A library with no choices in it.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLibrary;

impl ChoiceLibrary for NoLibrary {
    fn variables(&self, _option: &str, _choice: &str) -> Result<Option<toml::Table>, String> {
        Ok(None)
    }
}

/// Which choices are made, and where the choices a style does not hold itself
/// are.
#[derive(Clone, Copy)]
pub struct Selection<'selection> {
    /// The directory of the style's file, where its `file` choices are; none
    /// for a style read from text alone.
    pub directory: Option<&'selection Path>,
    /// The choice made for each option, by option; an option not named has
    /// its default.
    pub chosen: &'selection BTreeMap<String, String>,
    /// Where the shared choices are.
    pub library: &'selection dyn ChoiceLibrary,
}

impl std::fmt::Debug for Selection<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Selection")
            .field("directory", &self.directory)
            .field("chosen", &self.chosen)
            .finish_non_exhaustive()
    }
}

/// The options a style declares.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleOptions {
    /// Each option, sorted by name.
    pub options: Vec<StyleOption>,
}

/// One option of a style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleOption {
    /// Its name, as `style_options` names it.
    pub name: String,
    /// What to call it for people, if the style says.
    pub label: Option<String>,
    /// What to call it as a checkbox, if the style says and the option has
    /// two choices: checked, the one that is not the default is chosen.
    pub checkbox: Option<String>,
    /// The choice that shows the checkbox checked, if the style says; else
    /// the one that is not the default.
    pub checked: Option<String>,
    /// The group of options a menu or window lists it with, if the style says.
    pub group: Option<String>,
    /// The choice made when none is: always one the style holds itself.
    pub default: String,
    /// The choices the style holds itself: the default first, then the rest
    /// sorted by name.
    pub choices: Vec<String>,
    /// The group each of those choices says it is in (`group = "Blues"`), by
    /// choice, for a list that folds many choices into a few.
    pub groups: BTreeMap<String, String>,
    /// The choice in effect: the one asked for, or the default where there
    /// is no such choice.
    pub chosen: String,
}

/// The variables of a style, and what is noted about its options.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Resolved {
    /// The options declared.
    pub(super) options: StyleOptions,
    /// The variable values in effect.
    pub(super) variables: Variables,
    /// What was wrong with a choice made, a sentence each; the option's
    /// default is used instead.
    pub(super) problems: Vec<String>,
}

/// The value of each variable.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct Variables(HashMap<String, toml::Value>);

impl Eq for Variables {}

impl Variables {
    /// Returns the value of `name` with the variables it refers to put in, or
    /// why there is none. `chain` holds the variables being put in, so a
    /// variable that leads back to itself is told.
    fn get(&self, name: &str, chain: &mut Vec<String>) -> Result<toml::Value, String> {
        let value = self
            .0
            .get(name)
            .ok_or_else(|| format!("`${name}` is not a variable this style defines"))?;
        if chain.iter().any(|known| known == name) {
            let path = chain
                .iter()
                .chain(std::iter::once(&name.to_owned()))
                .map(|link| format!("${link}"))
                .collect::<Vec<_>>()
                .join(" -> ");
            return Err(format!("variables refer to each other in a circle: {path}"));
        }
        chain.push(name.to_owned());
        let value = self.substitute_in(value.clone(), chain);
        chain.pop();
        value
    }

    /// Returns `value` with every variable it refers to replaced: a string
    /// that is `"$name"` by the variable's value, whatever it is, and
    /// `$name` inside a string by the variable's text. A variable's value may
    /// itself refer to variables.
    pub(super) fn substitute(&self, value: toml::Value) -> Result<toml::Value, String> {
        self.substitute_in(value, &mut Vec::new())
    }

    /// [`Self::substitute`], inside the variables of `chain`.
    fn substitute_in(
        &self,
        value: toml::Value,
        chain: &mut Vec<String>,
    ) -> Result<toml::Value, String> {
        match value {
            toml::Value::String(text) => self.in_text(&text, chain),
            toml::Value::Array(items) => items
                .into_iter()
                .map(|item| self.substitute_in(item, chain))
                .collect::<Result<_, _>>()
                .map(toml::Value::Array),
            toml::Value::Table(table) => table
                .into_iter()
                .map(|(key, item)| Ok((key, self.substitute_in(item, chain)?)))
                .collect::<Result<toml::Table, String>>()
                .map(toml::Value::Table),
            other @ (toml::Value::Integer(_)
            | toml::Value::Float(_)
            | toml::Value::Boolean(_)
            | toml::Value::Datetime(_)) => Ok(other),
        }
    }

    /// Returns the value `text` stands for.
    fn in_text(&self, text: &str, chain: &mut Vec<String>) -> Result<toml::Value, String> {
        if !text.contains('$') {
            return Ok(toml::Value::String(text.to_owned()));
        }
        if let Some(name) = text.strip_prefix('$')
            && is_name(name)
        {
            return self.get(name, chain);
        }
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find('$') {
            out.push_str(&rest[..at]);
            let after = &rest[at + 1..];
            let length = after
                .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .unwrap_or(after.len());
            let name = &after[..length];
            if !is_name(name) {
                out.push('$');
                rest = after;
                continue;
            }
            match self.get(name, chain)? {
                toml::Value::String(part) => out.push_str(&part),
                toml::Value::Integer(part) => out.push_str(&part.to_string()),
                toml::Value::Float(part) => out.push_str(&part.to_string()),
                toml::Value::Boolean(_)
                | toml::Value::Datetime(_)
                | toml::Value::Array(_)
                | toml::Value::Table(_) => {
                    return Err(format!(
                        "`${name}` is not text or a number, so it cannot be part of text"
                    ));
                }
            }
            rest = &after[length..];
        }
        out.push_str(rest);
        Ok(toml::Value::String(out))
    }
}

/// Returns whether `name` is a name an option, choice or variable may have: a
/// letter or `_`, then letters, digits and `_`.
fn is_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Returns `name`, or the error that it is no name for what it names.
fn checked_name<'name>(name: &'name str, what: &str) -> Result<&'name str, toml::de::Error> {
    if is_name(name) {
        Ok(name)
    } else {
        Err(toml::de::Error::custom(format!(
            "`{name}` is no name for {what}: letters, digits and `_`, not starting with a digit"
        )))
    }
}

/// Keys of a choice's table that are not variables.
const CHOICE_KEYS: [&str; 3] = ["file", "group", "label"];

/// Reads the options of the style `text` and picks the variables of the
/// choices `selection` makes.
///
/// # Errors
///
/// Returns an error if the text is not TOML, or an option is declared wrongly:
/// no `default`, a default that is not inline, a name that is not one.
pub(super) fn resolve(text: &str, selection: &Selection<'_>) -> Result<Resolved, toml::de::Error> {
    let table: toml::Table = toml::from_str(text)?;
    let Some(declared) = table.get("options") else {
        return Ok(Resolved::default());
    };
    let declared = declared
        .as_table()
        .ok_or_else(|| toml::de::Error::custom("`options` is a table of options"))?;
    let mut resolved = Resolved::default();
    let mut owners: HashMap<String, String> = HashMap::new();
    for (name, option) in declared {
        checked_name(name, "an option")?;
        let option = option.as_table().ok_or_else(|| {
            toml::de::Error::custom(format!(
                "option `{name}` is a table: its default and choices"
            ))
        })?;
        let (declaration, variables) =
            read_option(name, option, selection, &mut resolved.problems)?;
        for (variable, value) in variables {
            if let Some(other) = owners.insert(variable.clone(), name.clone()) {
                return Err(toml::de::Error::custom(format!(
                    "the variable `{variable}` is defined by the options `{other}` and `{name}`: \
                     each variable belongs to one option"
                )));
            }
            resolved.variables.0.insert(variable, value);
        }
        resolved.options.options.push(declaration);
    }
    Ok(resolved)
}

/// Returns the names of the choices of the option `name`, in the order its
/// table has them, and the group each says it is in.
fn choices_of(
    name: &str,
    option: &toml::Table,
) -> Result<(Vec<String>, BTreeMap<String, String>), toml::de::Error> {
    let mut choices = Vec::new();
    let mut groups = BTreeMap::new();
    for (key, value) in option {
        if matches!(
            key.as_str(),
            "label" | "default" | "checkbox" | "checked" | "group"
        ) {
            continue;
        }
        checked_name(key, "a choice")?;
        if !value.is_table() {
            return Err(toml::de::Error::custom(format!(
                "option `{name}`: choice `{key}` is a table of variables"
            )));
        }
        if let Some(group) = value.get("group").and_then(toml::Value::as_str) {
            groups.insert(key.clone(), group.to_owned());
        }
        choices.push(key.clone());
    }
    Ok((choices, groups))
}

/// Reads one option's declaration and the variables of its chosen choice.
fn read_option(
    name: &str,
    option: &toml::Table,
    selection: &Selection<'_>,
    problems: &mut Vec<String>,
) -> Result<(StyleOption, toml::Table), toml::de::Error> {
    let fail = |message: String| toml::de::Error::custom(format!("option `{name}`: {message}"));
    let text_of = |key: &str| {
        option
            .get(key)
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    };
    let (label, checkbox, group, checked) = (
        text_of("label"),
        text_of("checkbox"),
        text_of("group"),
        text_of("checked"),
    );
    let default = text_of("default")
        .ok_or_else(|| fail("`default` names the choice made when none is".to_owned()))?;
    let (mut choices, groups) = choices_of(name, option)?;
    // The default first; a TOML table has no order to keep, so the rest are
    // sorted.
    choices.sort_by_key(|choice| (*choice != default, choice.clone()));
    let base = option
        .get(&default)
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            fail(format!(
                "the default, `{default}`, is not a choice written inline"
            ))
        })?;
    if base.contains_key("file") {
        return Err(fail(format!(
            "the default, `{default}`, is written inline, not in a file"
        )));
    }
    let base = variables_of(base);
    let wanted = selection
        .chosen
        .get(name)
        .cloned()
        .unwrap_or_else(|| default.clone());
    let mut variables = base.clone();
    let mut found = wanted == default;
    if wanted != default
        && let Some(own) = option.get(&wanted).and_then(toml::Value::as_table)
    {
        match own_variables(own, selection.directory) {
            Ok(own) => {
                layer(&mut variables, &own);
                found = true;
            }
            Err(message) => problems.push(format!("option `{name}`, choice `{wanted}`: {message}")),
        }
    }
    match selection.library.variables(name, &wanted) {
        Ok(Some(shared)) => {
            layer(&mut variables, &shared);
            found = true;
        }
        Ok(None) => {}
        Err(message) => problems.push(format!("option `{name}`, choice `{wanted}`: {message}")),
    }
    let chosen = if found {
        wanted
    } else {
        // The problem noted is the more specific one, if a choice was there
        // but unreadable; otherwise there is no such choice.
        if !problems
            .iter()
            .any(|problem| problem.starts_with(&format!("option `{name}`, choice `{wanted}`")))
        {
            problems.push(format!(
                "option `{name}` has no choice `{wanted}`; the default, `{default}`, is used"
            ));
        }
        variables = base;
        default.clone()
    };
    Ok((
        StyleOption {
            name: name.to_owned(),
            label,
            checkbox,
            checked,
            group,
            default,
            choices,
            groups,
            chosen,
        },
        variables,
    ))
}

/// Returns the variables a choice's table holds: all its keys but the ones
/// that say something else about it.
fn variables_of(table: &toml::Table) -> toml::Table {
    table
        .iter()
        .filter(|(key, _)| !CHOICE_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Returns the variables of a style's own choice: its table, or the file it
/// names beside the style.
fn own_variables(table: &toml::Table, directory: Option<&Path>) -> Result<toml::Table, String> {
    let Some(file) = table.get("file") else {
        return Ok(variables_of(table));
    };
    let name = file
        .as_str()
        .ok_or_else(|| "`file` names a file beside the style".to_owned())?;
    let directory = directory
        .ok_or_else(|| format!("`{name}`: this style is not a file, so it has no choice files"))?;
    let path = Path::new(name);
    // A choice file is beside the style, not anywhere on the disk.
    if path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(format!(
            "`{name}`: a choice file is a path inside the style's directory"
        ));
    }
    let text = std::fs::read_to_string(directory.join(path))
        .map_err(|error| format!("`{name}`: {error}"))?;
    toml::from_str::<toml::Table>(&text)
        .map(|read| variables_of(&read))
        .map_err(|error| format!("`{name}`: {}", error.message()))
}

/// Puts the variables of `above` over `variables`, for the ones `variables`
/// has: a choice may set only what the default choice defines.
fn layer(variables: &mut toml::Table, above: &toml::Table) {
    for (key, value) in above {
        if CHOICE_KEYS.contains(&key.as_str()) {
            continue;
        }
        if let Some(slot) = variables.get_mut(key) {
            *slot = value.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library of `(option, choice, TOML)` triples.
    struct Library(Vec<(&'static str, &'static str, &'static str)>);

    impl ChoiceLibrary for Library {
        fn variables(&self, option: &str, choice: &str) -> Result<Option<toml::Table>, String> {
            Ok(self
                .0
                .iter()
                .find(|(known, kind, _)| *known == option && *kind == choice)
                .map(|(.., text)| toml::from_str(text).expect("TOML")))
        }
    }

    const STYLE: &str = r#"
[options.scheme]
label = "Color scheme"
default = "graphite"
[options.scheme.graphite]
bar = "grey30"
line = "grey60"
[options.scheme.blue]
bar = "navy"
"#;

    fn pick(
        text: &str,
        chosen: &[(&str, &str)],
        library: &dyn ChoiceLibrary,
    ) -> Result<Resolved, toml::de::Error> {
        let chosen: BTreeMap<String, String> = chosen
            .iter()
            .map(|(option, choice)| ((*option).to_owned(), (*choice).to_owned()))
            .collect();
        resolve(
            text,
            &Selection {
                directory: None,
                chosen: &chosen,
                library,
            },
        )
    }

    fn value(resolved: &Resolved, name: &str) -> String {
        resolved
            .variables
            .get(name, &mut Vec::new())
            .expect("defined")
            .as_str()
            .expect("text")
            .to_owned()
    }

    #[test]
    fn the_default_choice_is_in_effect_when_none_is_made() {
        let resolved = pick(STYLE, &[], &NoLibrary).expect("valid");
        assert_eq!(value(&resolved, "bar"), "grey30");
        let option = &resolved.options.options[0];
        assert_eq!(
            (option.chosen.as_str(), option.default.as_str()),
            ("graphite", "graphite")
        );
        assert_eq!(option.choices, ["graphite", "blue"], "the default first");
        assert_eq!(option.label.as_deref(), Some("Color scheme"));
    }

    #[test]
    fn a_choice_that_leaves_variables_out_keeps_the_defaults() {
        let resolved = pick(STYLE, &[("scheme", "blue")], &NoLibrary).expect("valid");
        assert_eq!(value(&resolved, "bar"), "navy");
        assert_eq!(value(&resolved, "line"), "grey60", "the default's");
        assert_eq!(resolved.problems, Vec::<String>::new());
    }

    #[test]
    fn a_library_choice_wins_over_the_styles_own() {
        let library = Library(vec![("scheme", "blue", "bar = \"teal\"\nextra = \"x\"")]);
        let resolved = pick(STYLE, &[("scheme", "blue")], &library).expect("valid");
        assert_eq!(value(&resolved, "bar"), "teal");
        assert!(
            resolved.variables.get("extra", &mut Vec::new()).is_err(),
            "not a variable of the style"
        );
    }

    #[test]
    fn a_choice_only_the_library_has_can_be_made() {
        let library = Library(vec![("scheme", "mine", "line = \"red\"")]);
        let resolved = pick(STYLE, &[("scheme", "mine")], &library).expect("valid");
        assert_eq!(value(&resolved, "line"), "red");
        assert_eq!(value(&resolved, "bar"), "grey30");
        assert_eq!(resolved.options.options[0].chosen, "mine");
    }

    #[test]
    fn a_library_choice_may_replace_the_default_choice_too() {
        let library = Library(vec![("scheme", "graphite", "bar = \"black\"")]);
        let resolved = pick(STYLE, &[], &library).expect("valid");
        assert_eq!(value(&resolved, "bar"), "black");
    }

    #[test]
    fn a_choice_nothing_has_is_noted_and_the_default_used() {
        let resolved = pick(STYLE, &[("scheme", "nowhere")], &NoLibrary).expect("valid");
        assert_eq!(value(&resolved, "bar"), "grey30");
        assert_eq!(resolved.options.options[0].chosen, "graphite");
        assert_eq!(resolved.problems.len(), 1);
        assert!(
            resolved.problems[0].contains("nowhere"),
            "{:?}",
            resolved.problems
        );
    }

    #[test]
    fn a_choice_file_is_read_beside_the_style() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(directory.path().join("green.toml"), "bar = \"darkgreen\"\n")
            .expect("written");
        let text = format!("{STYLE}[options.scheme.green]\nfile = \"green.toml\"\n");
        let chosen = BTreeMap::from([("scheme".to_owned(), "green".to_owned())]);
        let resolved = resolve(
            &text,
            &Selection {
                directory: Some(directory.path()),
                chosen: &chosen,
                library: &NoLibrary,
            },
        )
        .expect("valid");
        assert_eq!(value(&resolved, "bar"), "darkgreen");
        assert_eq!(resolved.problems, Vec::<String>::new());
    }

    #[test]
    fn a_choice_file_outside_the_styles_directory_is_refused() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let text = format!("{STYLE}[options.scheme.evil]\nfile = \"../x.toml\"\n");
        let chosen = BTreeMap::from([("scheme".to_owned(), "evil".to_owned())]);
        let resolved = resolve(
            &text,
            &Selection {
                directory: Some(directory.path()),
                chosen: &chosen,
                library: &NoLibrary,
            },
        )
        .expect("valid");
        assert_eq!(resolved.problems.len(), 1, "{:?}", resolved.problems);
        assert_eq!(value(&resolved, "bar"), "grey30");
    }

    #[test]
    fn a_badly_declared_option_is_an_error() {
        for (text, mentions) in [
            ("[options.scheme]\n[options.scheme.a]\nx = 1\n", "default"),
            ("[options.scheme]\ndefault = \"a\"\n", "inline"),
            (
                "[options.scheme]\ndefault = \"a\"\n[options.scheme.a]\nfile = \"a.toml\"\n",
                "inline",
            ),
            ("[options.\"bad name\"]\ndefault = \"a\"\n", "no name"),
            (
                "[options.a]\ndefault = \"x\"\n[options.a.x]\nv = 1\n\
                 [options.b]\ndefault = \"y\"\n[options.b.y]\nv = 2\n",
                "belongs to one option",
            ),
        ] {
            let error = pick(text, &[], &NoLibrary)
                .expect_err("refused")
                .to_string();
            assert!(error.contains(mentions), "{text}: {error}");
        }
    }

    #[test]
    fn a_whole_value_is_the_variable_and_text_has_it_put_in() {
        let resolved = pick(
            "[options.size]\ndefault = \"m\"\n[options.size.m]\npad = 4\nbar = \"1lh + 2\"\n\
             texture = { solid = \"red\" }\n",
            &[],
            &NoLibrary,
        )
        .expect("valid");
        let variables = &resolved.variables;
        assert_eq!(
            variables.substitute("$pad".into()),
            Ok(toml::Value::Integer(4)),
            "a number stays a number"
        );
        assert_eq!(
            variables.substitute("$bar + $pad".into()),
            Ok(toml::Value::String("1lh + 2 + 4".to_owned()))
        );
        assert_eq!(
            variables.substitute("$texture".into()),
            Ok(toml::Value::Table(
                toml::from_str("solid = \"red\"").expect("TOML")
            ))
        );
        assert!(
            variables
                .substitute("$texture and".into())
                .expect_err("a table is not text")
                .contains("not text or a number")
        );
        assert!(
            variables
                .substitute("$nope".into())
                .expect_err("no such variable")
                .contains("`$nope`")
        );
        assert_eq!(
            variables.substitute("cost: 5$".into()),
            Ok(toml::Value::String("cost: 5$".to_owned())),
            "a dollar sign that names nothing is a dollar sign"
        );
    }

    /// A style whose `look` variable is made of the scheme's colors.
    const NESTED: &str = r#"
[options.scheme]
default = "graphite"
[options.scheme.graphite]
light = "grey55"
dark = "grey5"
[options.look]
default = "plain"
[options.look.plain]
face = { gradient = "vertical", from = "$light", to = "$dark" }
caption = "$face and $light"
"#;

    #[test]
    fn a_variable_may_be_made_of_others() {
        let resolved = pick(NESTED, &[], &NoLibrary).expect("valid");
        assert_eq!(
            resolved.variables.substitute("$face".into()),
            Ok(toml::Value::Table(
                toml::from_str("gradient = \"vertical\"\nfrom = \"grey55\"\nto = \"grey5\"")
                    .expect("TOML")
            ))
        );
    }

    #[test]
    fn a_choice_changing_an_inner_variable_changes_what_is_made_of_it() {
        let library = Library(vec![("scheme", "blue", "light = \"navy\"\n")]);
        let resolved = pick(NESTED, &[("scheme", "blue")], &library).expect("valid");
        let face = resolved
            .variables
            .substitute("$face".into())
            .expect("resolves");
        assert_eq!(face["from"].as_str(), Some("navy"));
    }

    #[test]
    fn variables_leading_back_to_themselves_are_an_error_naming_the_circle() {
        let resolved = pick(
            "[options.loop]\ndefault = \"a\"\n[options.loop.a]\none = \"$two\"\ntwo = \"$one\"\n",
            &[],
            &NoLibrary,
        )
        .expect("valid");
        let error = resolved
            .variables
            .substitute("$one".into())
            .expect_err("a circle");
        assert!(error.contains("$one -> $two -> $one"), "{error}");
    }

    #[test]
    fn text_has_a_variable_made_of_others_put_in() {
        let resolved = pick(
            "[options.s]\ndefault = \"a\"\n[options.s.a]\npad = 4\nbar = \"1lh + $pad\"\n",
            &[],
            &NoLibrary,
        )
        .expect("valid");
        assert_eq!(
            resolved.variables.substitute("x: $bar".into()),
            Ok(toml::Value::String("x: 1lh + 4".to_owned()))
        );
    }
}

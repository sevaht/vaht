//! The library of choices shared by every style: `options/<option>/<choice>.toml`
//! (02 §10.5).
//!
//! A choice file is a table of variables. It is found in the user's
//! configuration directory, then in the system's; a style that declares the
//! option and uses the same variable names takes it up.

use std::collections::BTreeMap;
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
    /// What to call it as a checkbox, if it has two choices.
    pub(crate) checkbox: Option<String>,
    /// The choice that shows the checkbox checked, if it says.
    pub(crate) checked: Option<String>,
    /// The group of options it is listed with, if the style says.
    pub(crate) group: Option<String>,
    /// The choice made when none is.
    pub(crate) default: String,
    /// The choice in effect.
    pub(crate) chosen: String,
    /// The choices: the style's own and the shared ones, the default first,
    /// the rest sorted.
    pub(crate) choices: Vec<String>,
    /// The group each choice says it is in (`group = "Blues"` in its table
    /// or file), by choice.
    pub(crate) groups: BTreeMap<String, String>,
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
                // An option may take its choices from another's directory.
                let directory = option.library.as_deref().unwrap_or(&option.name);
                let mut choices: Vec<String> = option
                    .choices
                    .iter()
                    .cloned()
                    .chain(self.choices(directory))
                    .collect();
                choices.sort_by_key(|choice| (*choice != option.default, choice.clone()));
                choices.dedup();
                // A file in the library says its group, over the style's own.
                let mut groups = option.groups.clone();
                for choice in &choices {
                    if let Ok(Some(table)) = self.variables(directory, choice)
                        && let Some(group) = table.get("group").and_then(toml::Value::as_str)
                    {
                        groups.insert(choice.clone(), group.to_owned());
                    }
                }
                ListedOption {
                    name: option.name.clone(),
                    label: option.label.clone(),
                    checkbox: option.checkbox.clone(),
                    checked: option.checked.clone(),
                    group: option.group.clone(),
                    default: option.default.clone(),
                    chosen: option.chosen.clone(),
                    choices,
                    groups,
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
    use vaht_ui::Color;
    use vaht_ui::text::TextEffect;
    use vaht_ui::texture::Fill;
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

    /// The shipped choices of `option`: their names and files.
    fn shipped(option: &str) -> Vec<(String, PathBuf)> {
        std::fs::read_dir(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../data/options")
                .join(option),
        )
        .expect("the shipped choices")
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
    fn every_shipped_palette_loads_with_the_default_style_and_looks_different() {
        let default = load(&[]);
        assert_eq!(default.option_problems(), Vec::<String>::new());
        let vocabulary: toml::Table = toml::from_str(Style::BUILT_IN).expect("TOML");
        let vocabulary = vocabulary["options"]["palette"]["graphite"]
            .as_table()
            .expect("the default palette");
        let shipped = shipped("palette");
        assert!(
            shipped.len() >= 14,
            "the shipped palettes: {}",
            shipped.len()
        );
        for (name, path) in shipped {
            let table: toml::Table =
                toml::from_str(&std::fs::read_to_string(&path).expect("readable")).expect("TOML");
            let style = load(&[("palette", &name)]);
            assert_eq!(style.option_problems(), Vec::<String>::new(), "{name}");
            assert_ne!(style, default, "{name} changes the look");
            // It sets only colors the default style has a name for.
            for key in table.keys().filter(|key| *key != "group") {
                assert!(
                    vocabulary.contains_key(key),
                    "{name} sets `{key}`, which the default style has no variable for"
                );
            }
        }
    }

    #[test]
    fn every_shipped_palette_is_a_choice_of_all_four_parts() {
        let palettes: Vec<String> = shipped("palette")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        for option in [
            "palette",
            "focused_palette",
            "unfocused_palette",
            "menu_palette",
        ] {
            for palette in &palettes {
                let style = load(&[(option, palette)]);
                assert_eq!(
                    style.option_problems(),
                    Vec::<String>::new(),
                    "{option} {palette}"
                );
                assert_eq!(
                    style
                        .options()
                        .options
                        .iter()
                        .find(|listed| listed.name == option)
                        .map(|listed| listed.chosen.as_str()),
                    Some(palette.as_str())
                );
            }
        }
    }

    #[test]
    fn every_other_choice_of_the_default_style_loads_and_looks_different() {
        let default = load(&[]);
        for (option, choice) in [
            ("focused_palette", "amber"),
            ("unfocused_palette", "amber"),
            ("menu_palette", "amber"),
            ("shading", "soft"),
            ("focused_shading", "soft"),
            ("unfocused_shading", "soft"),
            ("menu_shading", "soft"),
            ("taskbar_shading", "soft"),
            ("menu_body", "filled"),
            ("outline", "off"),
            ("taskbar_outline", "off"),
            ("focused_outline", "off"),
            ("unfocused_outline", "off"),
            ("menu_outline", "off"),
            ("titlebar_size", "small"),
            ("titlebar_size", "large"),
            ("taskbar_size", "small"),
            ("taskbar_size", "xlarge"),
            ("menu_size", "small"),
            ("menu_size", "large"),
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
    fn each_option_changes_what_it_names() {
        let element = |style: &Style, element, state| style.element_style(element, state).clone();
        let focused = |style: &Style, which| element(style, which, WidgetState::Focused);
        let default = load(&[]);
        let default_label = focused(&default, StyleElement::Label);
        assert_ne!(
            default_label.text.effect,
            TextEffect::None,
            "graphite has an outline"
        );
        // The outlines are on, and each is its own: the bars' and the menus'.
        let menu_title = |style: &Style| focused(style, StyleElement::MenuTitle).text.effect;
        let bar_label = |style: &Style| focused(style, StyleElement::Label).text.effect;
        assert_ne!(menu_title(&default), TextEffect::None);
        let no_bars = load(&[("focused_outline", "off")]);
        assert_eq!(bar_label(&no_bars), TextEffect::None);
        assert_eq!(
            focused(&no_bars, StyleElement::WindowListEntry).text.effect,
            TextEffect::None
        );
        let unfocused_label = |style: &Style| {
            element(style, StyleElement::Label, WidgetState::Unfocused)
                .text
                .effect
        };
        assert_ne!(unfocused_label(&no_bars), TextEffect::None, "its own part");
        assert_ne!(menu_title(&no_bars), TextEffect::None, "menus keep theirs");
        let no_menus = load(&[("menu_outline", "off")]);
        assert_eq!(menu_title(&no_menus), TextEffect::None);
        assert_eq!(
            element(&no_menus, StyleElement::MenuItem, WidgetState::Hovered)
                .text
                .effect,
            TextEffect::None,
            "and so do the marks, which have the item's"
        );
        assert_ne!(bar_label(&no_menus), TextEffect::None, "bars keep theirs");
        assert_eq!(
            focused(&no_bars, StyleElement::Label).texture,
            default_label.texture,
            "the outline is only the outline"
        );
        // Windows are square and menus rounded.
        let radius = |style: &Style, element| focused(style, element).corner_radius;
        assert_eq!(radius(&default, StyleElement::Border), 0);
        assert_eq!(radius(&default, StyleElement::MenuFrame), 8);
        // Window buttons are drawn on the titlebar.
        for state in [WidgetState::Focused, WidgetState::Unfocused] {
            assert_eq!(
                element(&default, StyleElement::Button, state).texture.fill,
                Fill::Parent
            );
        }
        // The faces are gradients, and a focused window's titlebar and the
        // taskbar's focused entry are one: the palette's accent.
        let titlebar = focused(&default, StyleElement::Titlebar).texture.fill;
        assert!(matches!(titlebar, Fill::Gradient { .. }));
        assert_eq!(
            titlebar,
            focused(&default, StyleElement::WindowListEntry)
                .texture
                .fill
        );
        assert!(matches!(
            focused(&default, StyleElement::Taskbar).texture.fill,
            Fill::Gradient { .. }
        ));
        // The title and the buttons are on the one face: the titlebar's.
        assert_eq!(
            focused(&default, StyleElement::Label).texture.fill,
            Fill::Parent
        );
        assert_eq!(
            focused(&default, StyleElement::Button).texture.fill,
            Fill::Parent
        );
        // Unfocused windows can be in another palette's colors: graphite's
        // greys, for one, whatever the palette, with the same spread.
        let unfocused = |style: &Style| {
            element(style, StyleElement::WindowListEntry, WidgetState::Unfocused).texture
        };
        assert_eq!(
            unfocused(&load(&[
                ("palette", "purple"),
                ("unfocused_palette", "graphite")
            ])),
            unfocused(&default)
        );
        // An unfocused window's border is its own color, not the focused one's.
        let orange = load(&[("palette", "orange")]);
        assert_ne!(
            element(&orange, StyleElement::Border, WidgetState::Focused)
                .border
                .color,
            element(&orange, StyleElement::Border, WidgetState::Unfocused)
                .border
                .color
        );
    }

    #[test]
    fn each_part_has_its_own_palette_outline_and_shading() {
        let element = |style: &Style, element, state| style.element_style(element, state).clone();
        let fill = |style: &Style, which, state| element(style, which, state).texture.fill;
        let default = load(&[]);
        let mixed = load(&[
            ("palette", "purple"),
            ("focused_palette", "amber"),
            ("unfocused_palette", "graphite"),
            ("menu_palette", "blue"),
        ]);
        assert_eq!(mixed.option_problems(), Vec::<String>::new());
        let titlebar = |style: &Style| fill(style, StyleElement::Titlebar, WidgetState::Focused);
        let purple_only = load(&[("palette", "purple")]);
        // Each part is the palette it was given and no other's.
        assert_ne!(titlebar(&mixed), titlebar(&purple_only));
        assert_eq!(
            titlebar(&mixed),
            titlebar(&load(&[("focused_palette", "amber")]))
        );
        assert_eq!(
            fill(&mixed, StyleElement::Titlebar, WidgetState::Unfocused),
            fill(&default, StyleElement::Titlebar, WidgetState::Unfocused)
        );
        assert_eq!(
            fill(&mixed, StyleElement::Taskbar, WidgetState::Focused),
            fill(&purple_only, StyleElement::Taskbar, WidgetState::Focused)
        );
        assert_eq!(
            element(&mixed, StyleElement::MenuFrame, WidgetState::Focused).border,
            element(
                &load(&[("menu_palette", "blue")]),
                StyleElement::MenuFrame,
                WidgetState::Focused
            )
            .border
        );
        // By default every part matches the taskbar's palette.
        let blue = load(&[("palette", "blue")]);
        let all_blue = load(&[
            ("palette", "blue"),
            ("focused_palette", "blue"),
            ("unfocused_palette", "blue"),
            ("menu_palette", "blue"),
        ]);
        assert_eq!(
            blue.element_style(StyleElement::Titlebar, WidgetState::Focused),
            all_blue.element_style(StyleElement::Titlebar, WidgetState::Focused)
        );
        assert_eq!(
            blue.element_style(StyleElement::MenuTitle, WidgetState::Focused),
            all_blue.element_style(StyleElement::MenuTitle, WidgetState::Focused)
        );
    }

    #[test]
    fn a_part_overrides_the_global_outline_and_shading_and_defaults_to_them() {
        let effect = |style: &Style, which, state| style.element_style(which, state).text.effect;
        let label = |style: &Style| effect(style, StyleElement::Label, WidgetState::Focused);
        let menu = |style: &Style| effect(style, StyleElement::MenuTitle, WidgetState::Focused);
        // Outlines are on by default; the global turns every part's off, and a
        // part that says on keeps its own.
        assert_ne!(label(&load(&[])), TextEffect::None);
        let off = load(&[("outline", "off")]);
        assert_eq!(label(&off), TextEffect::None);
        assert_eq!(menu(&off), TextEffect::None);
        let one = load(&[("outline", "off"), ("menu_outline", "on")]);
        assert_eq!(label(&one), TextEffect::None);
        assert_ne!(menu(&one), TextEffect::None);
        // Shading is deep by default; the global makes every part soft, and a
        // part that says deep stays deep.
        let fade =
            |style: &Style, which, state| match style.element_style(which, state).texture.fill {
                Fill::Gradient { to, .. } => to,
                Fill::Solid(_) | Fill::Image { .. } | Fill::Parent => panic!("a gradient"),
            };
        let deep = load(&[("palette", "blue")]);
        let soft = load(&[("palette", "blue"), ("shading", "soft")]);
        let mixed = load(&[
            ("palette", "blue"),
            ("shading", "soft"),
            ("menu_shading", "deep"),
        ]);
        for which in [StyleElement::Titlebar, StyleElement::MenuTitle] {
            assert_ne!(
                fade(&deep, which, WidgetState::Focused),
                fade(&soft, which, WidgetState::Focused)
            );
        }
        assert_eq!(
            fade(&mixed, StyleElement::Titlebar, WidgetState::Focused),
            fade(&soft, StyleElement::Titlebar, WidgetState::Focused)
        );
        assert_eq!(
            fade(&mixed, StyleElement::MenuTitle, WidgetState::Focused),
            fade(&deep, StyleElement::MenuTitle, WidgetState::Focused)
        );
        // A part can be soft alone.
        let alone = load(&[("palette", "blue"), ("focused_shading", "soft")]);
        assert_eq!(
            fade(&alone, StyleElement::Titlebar, WidgetState::Focused),
            fade(&soft, StyleElement::Titlebar, WidgetState::Focused)
        );
        assert_eq!(
            fade(&alone, StyleElement::MenuTitle, WidgetState::Focused),
            fade(&deep, StyleElement::MenuTitle, WidgetState::Focused)
        );
    }

    #[test]
    fn a_menus_title_follows_the_chrome_and_its_contents_are_their_own_choices() {
        let element = |style: &Style, element, state| style.element_style(element, state).clone();
        let focused = |style: &Style, which| element(style, which, WidgetState::Focused);
        let green = load(&[("palette", "green")]);
        // A menu's title is a focused face, as the taskbar's entry is.
        let title = focused(&green, StyleElement::MenuTitle).texture.fill;
        assert_eq!(
            title,
            focused(&green, StyleElement::WindowListEntry).texture.fill
        );
        // A menu's body is lighter only when asked.
        let body = |choice| {
            let style = load(&[("menu_body", choice)]);
            focused(&style, StyleElement::MenuFrame).texture
        };
        assert_ne!(body("filled"), body("dark"));
        // The selected item has its own bright color for its text and marks,
        // from the palette, on each body.
        let selected = |palette, body| {
            let style = load(&[("palette", palette), ("menu_body", body)]);
            element(&style, StyleElement::MenuItem, WidgetState::Hovered)
        };
        for body in ["dark", "filled"] {
            assert_ne!(
                selected("amber", body).text.color,
                selected("purple", body).text.color,
                "each palette has its own on a {body} menu"
            );
            assert_eq!(
                selected("amber", body).glyph_color,
                selected("amber", body).text.color,
                "its marks are its text's color"
            );
        }
        assert_ne!(
            selected("amber", "dark").text.color,
            selected("amber", "filled").text.color,
            "and one for each body"
        );
    }

    #[test]
    fn soft_shading_fades_to_a_muted_dark_in_any_palette_and_deep_to_near_black() {
        let fade = |palette, shading| {
            let style = load(&[("palette", palette), ("shading", shading)]);
            match style
                .element_style(StyleElement::Titlebar, WidgetState::Focused)
                .texture
                .fill
            {
                Fill::Gradient { to, .. } => to,
                Fill::Solid(_) | Fill::Image { .. } | Fill::Parent => panic!("a gradient"),
            }
        };
        for palette in ["blue", "cyan", "amber", "graphite"] {
            let (deep, soft) = (fade(palette, "deep"), fade(palette, "soft"));
            assert!(
                luminance(soft) > luminance(deep) * 2.0,
                "{palette}: the soft fade is not as dark ({deep:?} against {soft:?})"
            );
        }
        // Blue's soft shading is the muted blue-grey it used to be.
        assert_eq!(fade("blue", "soft"), Color::rgb(0x2e, 0x34, 0x40));
    }

    #[test]
    fn menu_marks_are_the_modern_ones() {
        let bullet = |state| {
            load(&[])
                .element_style(StyleElement::MenuItem, state)
                .bullet
        };
        assert_eq!(
            bullet(WidgetState::Focused),
            vaht_ui::style::Bullet::Chevron
        );
        assert_eq!(
            bullet(WidgetState::Hovered),
            vaht_ui::style::Bullet::DoubleChevron
        );
    }

    #[test]
    fn every_choice_of_every_option_works_with_every_palette() {
        let default = load(&[]);
        for option in &default.options().options {
            for choice in &option.choices {
                for (palette, _) in shipped("palette") {
                    let style = load(&[(option.name.as_str(), choice), ("palette", &palette)]);
                    assert_eq!(
                        style.option_problems(),
                        Vec::<String>::new(),
                        "{} = {choice} with {palette}",
                        option.name
                    );
                }
            }
        }
    }

    /// The relative luminance of `color`, 0 (black) to 1 (white).
    fn luminance(color: Color) -> f64 {
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.0722_f64.mul_add(
            channel(color.blue),
            0.2126_f64.mul_add(channel(color.red), 0.7152 * channel(color.green)),
        )
    }

    /// The contrast of two colors, 1 (none) to 21.
    fn contrast(first: Color, second: Color) -> f64 {
        let (first, second) = (luminance(first), luminance(second));
        (first.max(second) + 0.05) / (first.min(second) + 0.05)
    }

    /// Returns the colors of `style`'s element in `state` from end to end of
    /// its face; none for a face that is not colors.
    fn face_of(style: &Style, element: StyleElement, state: WidgetState) -> Option<Vec<Color>> {
        match style.element_style(element, state).texture.fill {
            Fill::Solid(color) => Some(vec![color]),
            Fill::Gradient { from, to, .. } => Some(vec![from, to]),
            Fill::Image { .. } | Fill::Parent => None,
        }
    }

    /// Returns `color` over `behind`, as it is drawn.
    fn over(color: Color, behind: Color) -> Color {
        let alpha = f64::from(color.alpha) / 255.0;
        let mix = |front: u8, back: u8| {
            let mixed = f64::from(front).mul_add(alpha, f64::from(back) * (1.0 - alpha));
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a mix of two bytes is a byte"
            )]
            let byte = mixed.round() as u8;
            byte
        };
        Color::rgb(
            mix(color.red, behind.red),
            mix(color.green, behind.green),
            mix(color.blue, behind.blue),
        )
    }

    /// Text is readable on the faces it is drawn on, for every palette in
    /// every look: at least this much contrast against every end of the face.
    const READABLE: f64 = 3.0;

    /// How much an outline must contrast with its text to carry it, and how
    /// much a menu item's text on a tint over its menu needs: its color is the
    /// only thing that says it is selected.
    const OUTLINED: f64 = 4.5;

    /// The text of `text_element` on the face of `face_element` in `state`
    /// that cannot be read, as a sentence, if it cannot.
    fn unreadable_text(
        style: &Style,
        (face_element, text_element): (StyleElement, StyleElement),
        state: WidgetState,
    ) -> Option<String> {
        let mut face = face_of(style, face_element, state)?;
        let looks = style.element_style(text_element, state);
        let (text, effect) = (looks.text.color, looks.text.effect);
        let mut needed = READABLE;
        // A face that lets what is behind it show is on the menu's, and
        // nothing else says the item is selected: its text must stand out.
        if face.iter().any(|color| color.alpha < u8::MAX)
            && let Some(frame) = face_of(style, StyleElement::MenuFrame, WidgetState::Focused)
        {
            face = face.iter().map(|color| over(*color, frame[0])).collect();
            needed = OUTLINED;
        }
        let worst = face
            .iter()
            .map(|color| contrast(*color, text))
            .fold(f64::INFINITY, f64::min);
        // An outline that stands out from the text carries it on any face; one
        // that does not (dark text, black outline) only smears it.
        let is_outlined = matches!(
            effect,
            TextEffect::Halo(halo) if contrast(halo, text) >= OUTLINED
        );
        (worst < needed && !(is_outlined && needed == READABLE))
            .then(|| format!("{text_element:?} {state:?} on {face_element:?} {worst:.1}:1"))
    }

    #[test]
    fn text_is_readable_on_its_face_in_every_palette_and_design() {
        let mut unreadable = Vec::new();
        // The unfocused colors of the palette, of graphite and of blue, whose
        // muted ones are the least like the rest.
        let designs = [
            [("unfocused_palette", "match_taskbar"), ("shading", "deep")],
            [("unfocused_palette", "graphite"), ("shading", "deep")],
            [("unfocused_palette", "blue"), ("shading", "deep")],
            [("unfocused_palette", "match_taskbar"), ("shading", "soft")],
            [("unfocused_palette", "graphite"), ("shading", "soft")],
            [("unfocused_palette", "blue"), ("shading", "soft")],
            // The other parts in another palette than the taskbar's.
            [("focused_palette", "graphite"), ("shading", "deep")],
            [("menu_palette", "blue"), ("shading", "soft")],
        ];
        let menus = [[("menu_body", "dark")], [("menu_body", "filled")]];
        let pairs = [
            (
                StyleElement::Titlebar,
                StyleElement::Label,
                WidgetState::Focused,
            ),
            (
                StyleElement::Titlebar,
                StyleElement::Label,
                WidgetState::Unfocused,
            ),
            (
                StyleElement::WindowListEntry,
                StyleElement::WindowListEntry,
                WidgetState::Focused,
            ),
            (
                StyleElement::WindowListEntry,
                StyleElement::WindowListEntry,
                WidgetState::Unfocused,
            ),
            (
                StyleElement::MenuItem,
                StyleElement::MenuItem,
                WidgetState::Hovered,
            ),
            (
                StyleElement::MenuTitle,
                StyleElement::MenuTitle,
                WidgetState::Focused,
            ),
            (
                StyleElement::MenuFrame,
                StyleElement::MenuItem,
                WidgetState::Focused,
            ),
        ];
        for (palette, _) in shipped("palette") {
            for design in designs {
                for menu in menus {
                    let mut chosen = vec![("palette", palette.as_str())];
                    chosen.extend(design);
                    chosen.extend(menu);
                    let style = load(&chosen);
                    for (face, text, state) in pairs {
                        if let Some(problem) = unreadable_text(&style, (face, text), state) {
                            unreadable.push(format!("{chosen:?}: {problem}"));
                        }
                    }
                }
            }
        }
        unreadable.sort();
        unreadable.dedup();
        assert!(unreadable.is_empty(), "{}", unreadable.join("\n"));
    }
}

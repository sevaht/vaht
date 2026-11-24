//! Variants of one Fluxbox style, folded into one vaht style with options.
//!
//! A style author often ships a style in many colors as many files: a folder
//! of `Accents-blue_red`, `Accents-blue_pink` and so on, or a style and its
//! `-Sakura` and `-Bubbles` variants. Converted one by one, each is a whole
//! style that differs from the others in a few values. vaht styles can offer
//! choices (02 §10.5), so those are folded: the values that differ become
//! variables, and each part of the variants' names (the `blue` and the `red`)
//! becomes an option, its values the choices.
//!
//! A group is folded only when it holds up: its styles are the same but for
//! values, and each value that differs depends on one part of the name alone
//! (where it would depend on two, no option can say it). Anything else is left
//! as the separate styles it was.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, TableLike, Value};

use crate::ConvertedStyle;
use crate::style::StyleImage;

/// A value of a style that differs between its variants: where it is, in
/// its tables.
type ValuePath = Vec<String>;

/// A converted style, with its document if it reads as one.
struct Entry {
    /// The converted style.
    style: ConvertedStyle,
    /// Its document.
    document: Option<DocumentMut>,
}

/// The values of a document that are not tables, with where they are; none
/// if it holds something that cannot be folded (an array of tables).
fn values_of(document: &DocumentMut) -> Option<BTreeMap<ValuePath, String>> {
    let mut found = BTreeMap::new();
    collect(document.as_table(), &mut Vec::new(), &mut found)?;
    Some(found)
}

/// Adds the values under `table` to `found`; none for an array of tables.
fn collect(
    table: &dyn TableLike,
    path: &mut ValuePath,
    found: &mut BTreeMap<ValuePath, String>,
) -> Option<()> {
    for (key, item) in table.iter() {
        path.push(key.to_owned());
        if let Some(inner) = item.as_table_like() {
            collect(inner, path, found)?;
        } else if let Item::Value(value) = item {
            found.insert(path.clone(), value.to_string().trim().to_owned());
        } else {
            return None;
        }
        path.pop();
    }
    Some(())
}

/// Replaces the value of `document` at `path` with `value`.
fn replace_at(document: &mut DocumentMut, path: &[String], value: Value) -> Option<()> {
    let (last, parents) = path.split_last()?;
    let mut table: &mut dyn TableLike = document.as_table_mut();
    for key in parents {
        table = table.get_mut(key)?.as_table_like_mut()?;
    }
    *table.get_mut(last)? = Item::Value(value);
    Some(())
}

/// Returns `text` as a name for a variable or option: lower case letters,
/// digits and `_`.
fn plain_name(text: &str) -> String {
    let name: String = text
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    name.trim_matches('_').to_owned()
}

/// Returns how bright the color `value` is (its text as a TOML value, such as
/// `"#214478"`), if it is a `#rrggbb` color.
fn brightness_of(value: &str) -> Option<f64> {
    let hex = value.trim().trim_matches('"').strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel =
        |range: std::ops::Range<usize>| u8::from_str_radix(hex.get(range)?, 16).ok().map(f64::from);
    Some(0.299 * channel(0..2)? + 0.587 * channel(2..4)? + 0.114 * channel(4..6)?)
}

/// Returns `part` of a style's name as the name of a choice: letters, digits
/// and `_`, not starting with a digit.
fn choice_name(part: &str) -> String {
    let name: String = part
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();
    // `default` and `label` are an option's own keys.
    if name.starts_with(|first: char| first.is_ascii_digit()) {
        format!("v{name}")
    } else if matches!(name.as_str(), "default" | "label") {
        format!("{name}_")
    } else {
        name
    }
}

/// Returns `name` capitalized, with `-` and `_` as spaces, for an option's
/// label.
fn label_of(name: &str) -> String {
    let spaced = name.replace(['-', '_'], " ");
    let mut characters = spaced.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

/// Folds the variants among `styles` into one style each; everything else
/// stays as it is, in order (a folded style where its first variant was).
/// `option_names` names the options of a family whose names have several
/// parts (`blue_red`), in order; the missing ones are `variant_1`,
/// `variant_2` and so on.
#[must_use]
pub fn fold_variants(styles: Vec<ConvertedStyle>, option_names: &[String]) -> Vec<ConvertedStyle> {
    let entries: Vec<Entry> = styles
        .into_iter()
        .map(|style| {
            let document = style.text.parse::<DocumentMut>().ok();
            Entry { style, document }
        })
        .collect();
    let mut claimed = vec![false; entries.len()];
    // The folded style, by the index of its first variant.
    let mut folded: BTreeMap<usize, ConvertedStyle> = BTreeMap::new();
    for group in groups(&entries, option_names) {
        if group.members.iter().any(|member| claimed[*member]) {
            continue;
        }
        if let Some(style) = fold_group(&group, &entries) {
            for member in &group.members {
                claimed[*member] = true;
            }
            let first = group.members.iter().copied().min().unwrap_or_default();
            folded.insert(first, style);
        }
    }
    entries
        .into_iter()
        .enumerate()
        .filter_map(|(index, entry)| match folded.remove(&index) {
            Some(style) => Some(style),
            None => (!claimed[index]).then_some(entry.style),
        })
        .collect()
}

/// A family of variants that may fold.
struct Group {
    /// The variants' indexes.
    members: Vec<usize>,
    /// The name of the folded style.
    family: String,
    /// The options' names.
    names: Vec<String>,
}

/// Returns the families among `variants`: by their names and where they came
/// from. Each has at least two variants, whose names tell them apart by the
/// parts after the family's name.
fn groups(entries: &[Entry], option_names: &[String]) -> Vec<Group> {
    let named: Vec<(usize, &Entry)> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.document.is_some())
        .collect();
    let mut found = Vec::new();
    // Siblings with `_` in their names, sharing the start of them:
    // `Accents-blue_red`.
    let mut by_parent: BTreeMap<PathBuf, Vec<(usize, &Entry)>> = BTreeMap::new();
    for (index, variant) in &named {
        if variant.style.name.contains('_') {
            by_parent
                .entry(parent_of(&variant.style.source))
                .or_default()
                .push((*index, variant));
        }
    }
    for siblings in by_parent.values() {
        if let Some(group) = underscore_family(siblings, option_names) {
            found.push(group);
        }
    }
    // A style and its `<style>-<word>` variants: `RaceFace`,
    // `RaceFace-Sakura`.
    for (index, base) in &named {
        let prefix = format!("{}-", base.style.name);
        let others: Vec<usize> = named
            .iter()
            .filter(|(other, variant)| {
                other != index
                    && parent_of(&variant.style.source) == parent_of(&base.style.source)
                    && variant.style.name.starts_with(&prefix)
                    && !variant.style.name.contains('_')
            })
            .map(|(other, _)| *other)
            .collect();
        if !others.is_empty() {
            let mut members = vec![*index];
            members.extend(others);
            found.push(Group {
                members,
                family: base.style.name.clone(),
                names: vec![
                    option_names
                        .first()
                        .map_or_else(|| "variant".to_owned(), |name| choice_name(name)),
                ],
            });
        }
    }
    found
}

/// Returns the directory a style came from.
fn parent_of(source: &Path) -> PathBuf {
    source.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Returns the family of `siblings` that share the start of their names and
/// differ by parts joined with `_`, if there is one.
fn underscore_family(siblings: &[(usize, &Entry)], option_names: &[String]) -> Option<Group> {
    if siblings.len() < 2 {
        return None;
    }
    let names: Vec<&str> = siblings
        .iter()
        .map(|(_, variant)| variant.style.name.as_str())
        .collect();
    let mut prefix = names.first()?.to_string();
    for name in &names {
        let shared = prefix
            .chars()
            .zip(name.chars())
            .take_while(|(one, other)| one == other)
            .count();
        prefix = prefix.chars().take(shared).collect();
    }
    // The family's name ends where a separator does.
    let cut = prefix.rfind(['-', '_'])?;
    let family = prefix[..cut].to_owned();
    let stem = &prefix[..=cut];
    if family.is_empty() {
        return None;
    }
    let part_counts: BTreeSet<usize> = names
        .iter()
        .map(|name| name[stem.len()..].split('_').count())
        .collect();
    let [count] = part_counts.into_iter().collect::<Vec<_>>()[..] else {
        return None;
    };
    Some(Group {
        members: siblings.iter().map(|(index, _)| *index).collect(),
        family,
        names: (0..count)
            .map(|position| {
                option_names.get(position).map_or_else(
                    || format!("variant_{}", position + 1),
                    |name| choice_name(name),
                )
            })
            .collect(),
    })
}

/// Folds the variants of `group` among `entries` into one style, if they
/// fold.
fn fold_group(group: &Group, entries: &[Entry]) -> Option<ConvertedStyle> {
    let (family, names) = (group.family.as_str(), &group.names);
    let variants: Vec<&Entry> = group.members.iter().map(|index| &entries[*index]).collect();
    let parts: Vec<Vec<String>> = variants
        .iter()
        .map(|variant| parts_of(&variant.style.name, family, names.len()))
        .collect::<Option<_>>()?;
    let values: Vec<BTreeMap<ValuePath, String>> = variants
        .iter()
        .map(|variant| values_of(variant.document.as_ref()?))
        .collect::<Option<_>>()?;
    let first = values.first()?;
    // The same values, in the same places.
    if values.iter().any(|other| !other.keys().eq(first.keys())) {
        return None;
    }
    // Which part of the name each differing value follows.
    let mut folds: Vec<BTreeMap<ValuePath, Vec<String>>> = vec![BTreeMap::new(); names.len()];
    let choices: Vec<Vec<String>> = (0..names.len())
        .map(|position| {
            let mut seen: Vec<String> = parts.iter().map(|part| part[position].clone()).collect();
            seen.sort();
            seen.dedup();
            seen
        })
        .collect();
    for path in first.keys() {
        let all: BTreeSet<&String> = values.iter().filter_map(|map| map.get(path)).collect();
        if all.len() < 2 {
            continue;
        }
        let owner = (0..names.len()).find(|position| {
            let mut by_choice: BTreeMap<&String, &String> = BTreeMap::new();
            parts.iter().zip(&values).all(|(part, map)| {
                let value = &map[path];
                *by_choice.entry(&part[*position]).or_insert(value) == value
            })
        })?;
        let by_choice: BTreeMap<&String, &String> = parts
            .iter()
            .zip(&values)
            .map(|(part, map)| (&part[owner], &map[path]))
            .collect();
        folds[owner].insert(
            path.clone(),
            choices[owner]
                .iter()
                .map(|choice| by_choice[choice].clone())
                .collect(),
        );
    }
    if folds.iter().all(BTreeMap::is_empty) {
        return None;
    }
    build(&variants, family, names, &choices, &folds)
}

/// Returns the parts of `name` after `family` and its separator, `count` of
/// them (joined with `_`; a single part may hold `-`), or for a style named
/// `family` itself, `original`.
fn parts_of(name: &str, family: &str, count: usize) -> Option<Vec<String>> {
    if name == family && count == 1 {
        return Some(vec!["original".to_owned()]);
    }
    let rest = name.strip_prefix(family)?;
    let rest = rest.strip_prefix(['-', '_'])?;
    let parts: Vec<String> = rest.split('_').map(choice_name).collect();
    (parts.len() == count && parts.iter().all(|part| !part.is_empty())).then_some(parts)
}

/// Writes the folded style: the first variant with its differing values
/// replaced by variables, and the options.
fn build(
    variants: &[&Entry],
    family: &str,
    names: &[String],
    choices: &[Vec<String>],
    folds: &[BTreeMap<ValuePath, Vec<String>>],
) -> Option<ConvertedStyle> {
    let base = variants.first()?;
    let mut document = base.document.clone()?;
    let mut options = String::new();
    let mut used: BTreeSet<String> = BTreeSet::new();
    for (position, option) in names.iter().enumerate() {
        // The paths that follow the same values in every choice share a
        // variable.
        let mut variables: Vec<(String, Vec<ValuePath>, Vec<String>)> = Vec::new();
        for (path, values) in &folds[position] {
            if let Some(known) = variables.iter_mut().find(|(_, _, other)| other == values) {
                known.1.push(path.clone());
                continue;
            }
            // Named after where it is first used, unless the values are
            // all colors: then by shade, below.
            let base_name = plain_name(&path.join("_"));
            let mut name = base_name.clone();
            let mut number = 2;
            while !used.insert(name.clone()) {
                name = format!("{base_name}_{number}");
                number += 1;
            }
            variables.push((name, vec![path.clone()], values.clone()));
        }
        // A palette: its variables are shades, named from the darkest.
        let brightness: Option<Vec<f64>> = variables
            .iter()
            .map(|(_, _, values)| values.first().and_then(|value| brightness_of(value)))
            .collect();
        if let Some(brightness) = brightness.filter(|brightness| brightness.len() > 1) {
            for (name, _, _) in &variables {
                used.remove(name);
            }
            let mut order: Vec<usize> = (0..variables.len()).collect();
            order.sort_by(|one, other| brightness[*one].total_cmp(&brightness[*other]));
            for (rank, index) in order.into_iter().enumerate() {
                let name = format!("{option}_{}", rank + 1);
                used.insert(name.clone());
                variables[index].0 = name;
            }
            variables.sort_by(|(one, _, _), (other, _, _)| one.cmp(other));
        }
        let default = choices[position].first()?;
        let _ = write!(
            options,
            "\n[options.{option}]\nlabel = \"{}\"\ndefault = \"{default}\"\n",
            label_of(option)
        );
        for (index, choice) in choices[position].iter().enumerate() {
            let _ = writeln!(options, "[options.{option}.{choice}]");
            for (name, _, values) in &variables {
                let _ = writeln!(options, "{name} = {}", values[index]);
            }
        }
        for (name, paths, _) in &variables {
            for path in paths {
                replace_at(&mut document, path, Value::from(format!("${name}")))?;
            }
        }
    }
    let folded_names = variants
        .iter()
        .map(|variant| variant.style.name.as_str())
        .collect::<Vec<_>>();
    let mut images: Vec<StyleImage> = Vec::new();
    for variant in variants {
        for image in &variant.style.images {
            if !images.iter().any(|known| known.name == image.name) {
                images.push(image.clone());
            }
        }
    }
    let header = format!(
        "# Converted from {} variants ({}) by vaht import fluxbox-style, folded into one style: the\n\
         # values that differ are variables, and the options below choose them.\n\n",
        variants.len(),
        folded_names.join(", ")
    );
    let body = document.to_string();
    let body = body
        .lines()
        .skip_while(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let text = format!("{header}{body}\n{options}");

    Some(ConvertedStyle {
        source: variants
            .first()
            .map(|variant| parent_of(&variant.style.source))
            .unwrap_or_default(),
        name: family.to_owned(),
        text,
        images,
        report: base.style.report.clone(),
        variants: variants.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Report;

    /// A variant named `name`, in `directory`, with the style `text`.
    fn variant(directory: &str, name: &str, text: &str) -> ConvertedStyle {
        ConvertedStyle {
            source: PathBuf::from(directory).join(name),
            name: name.to_owned(),
            text: text.to_owned(),
            images: Vec::new(),
            report: Report::default(),
            variants: 1,
        }
    }

    /// A style with a base color, an accent color and a size nothing varies.
    fn colors(base: &str, accent: &str) -> String {
        format!(
            "[default]\nfont = {{ size = 10 }}\n[window.titlebar]\ntexture = {{ solid = \"{base}\" }}\n\
             [window.label]\ntexture = {{ solid = \"{accent}\" }}\n[taskbar]\ntexture = {{ solid = \"{base}\" }}\n"
        )
    }

    fn family() -> Vec<ConvertedStyle> {
        let mut styles = Vec::new();
        for base in ["red", "blue"] {
            for accent in ["green", "gold", "pink"] {
                if base != accent {
                    styles.push(variant(
                        "/themes/Pack",
                        &format!("Pack-{base}_{accent}"),
                        &colors(&format!("#{base}"), &format!("#{accent}")),
                    ));
                }
            }
        }
        styles
    }

    #[test]
    fn variants_that_differ_in_two_parts_fold_into_two_options() {
        let folded = fold_variants(family(), &["base".to_owned(), "accent".to_owned()]);
        assert_eq!(folded.len(), 1);
        let style = &folded[0];
        assert_eq!(style.name, "Pack");
        assert_eq!(style.variants, 6);
        assert!(style.text.contains("[options.base]"), "{}", style.text);
        assert!(style.text.contains("[options.accent]"));
        assert!(style.text.contains("[options.base.red]"));
        assert!(style.text.contains("[options.accent.gold]"));
        // Two places use the base color and share one variable.
        assert_eq!(
            style.text.matches("\"$taskbar_texture_solid\"").count(),
            2,
            "the titlebar and the taskbar follow the base color: {}",
            style.text
        );
    }

    #[test]
    fn a_folded_style_gives_each_variant_back() {
        let styles = family();
        let originals: Vec<(String, String)> = styles
            .iter()
            .map(|style| (style.name.clone(), style.text.clone()))
            .collect();
        let folded = fold_variants(styles, &["base".to_owned(), "accent".to_owned()]);
        let document: DocumentMut = folded[0].text.parse().expect("a document");
        for (name, text) in originals {
            let (base, accent) = name
                .trim_start_matches("Pack-")
                .split_once('_')
                .expect("two parts");
            let original: DocumentMut = text.parse().expect("a document");
            let titlebar = original["window"]["titlebar"]["texture"]["solid"].as_str();
            let chosen = document["options"]["base"][base]["taskbar_texture_solid"].as_str();
            assert_eq!(chosen, titlebar, "{name}");
            let label = original["window"]["label"]["texture"]["solid"].as_str();
            let chosen =
                document["options"]["accent"][accent]["window_label_texture_solid"].as_str();
            assert_eq!(chosen, label, "{name}");
        }
    }

    #[test]
    fn a_value_that_follows_both_parts_cannot_fold() {
        let mut styles = family();
        // The taskbar follows the pair, which no one option can say.
        for style in &mut styles {
            let pair = style.name.trim_start_matches("Pack-").to_owned();
            style.text = style.text.replace(
                "[taskbar]\ntexture = { solid = \"#",
                &format!("[taskbar]\ntexture = {{ solid = \"{pair}#"),
            );
        }
        let folded = fold_variants(styles, &[]);
        assert_eq!(folded.len(), 6, "left as they were");
    }

    #[test]
    fn styles_that_are_not_alike_stay_separate() {
        let mut styles = family();
        styles[0]
            .text
            .push_str("[menu.title]\ntexture = { solid = \"#000\" }\n");
        let folded = fold_variants(styles, &[]);
        assert_eq!(folded.len(), 6);
    }

    #[test]
    fn a_style_and_its_suffixed_variants_fold_into_one_option() {
        let styles = vec![
            variant("/themes", "Race", &colors("#aaa", "#bbb")),
            variant("/themes", "Race-Sakura", &colors("#pink", "#bbb")),
            variant("/themes", "Race-Bubbles", &colors("#blue", "#bbb")),
            variant("/themes", "Other", &colors("#000", "#111")),
        ];
        let folded = fold_variants(styles, &[]);
        let names: Vec<&str> = folded.iter().map(|style| style.name.as_str()).collect();
        assert_eq!(
            names,
            ["Race", "Other"],
            "in order, the family where its first was"
        );
        assert!(folded[0].text.contains("[options.variant.original]"));
        assert!(folded[0].text.contains("[options.variant.Sakura]"));
        assert!(folded[0].text.contains("default = \"Bubbles\""));
    }

    #[test]
    fn colors_that_differ_are_named_by_shade_from_the_darkest() {
        let make = |name: &str, dark: &str, light: &str| {
            variant(
                "/themes/Pack",
                name,
                &format!(
                    "[window.titlebar]\ntexture = {{ solid = \"{dark}\" }}\n[window.label]\ntexture = {{ solid = \"{light}\" }}\n"
                ),
            )
        };
        let styles = vec![
            make("Pack", "#101010", "#c0c0c0"),
            make("Pack-red", "#200000", "#ff8080"),
            make("Pack-blue", "#000020", "#8080ff"),
        ];
        let folded = fold_variants(styles, &[]);
        let text = &folded[0].text;
        assert!(
            text.contains(
                "[options.variant.blue]\nvariant_1 = \"#000020\"\nvariant_2 = \"#8080ff\""
            ),
            "{text}"
        );
        assert!(
            text.contains("texture = { solid = \"$variant_1\" }"),
            "{text}"
        );
    }

    #[test]
    fn a_lone_style_is_left_alone() {
        let styles = vec![variant("/themes", "One", &colors("#aaa", "#bbb"))];
        assert_eq!(fold_variants(styles.clone(), &[]), styles);
    }
}

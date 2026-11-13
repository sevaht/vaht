//! Reading theme files (02 §10.1): TOML tables into [`ThemeLayers`].
//!
//! A theme file's tables are its layers: `[default]`, `[state.<state>]`,
//! `[<element>]` and `[<element>.<state>]`, where an element is a dotted
//! path such as `window.titlebar`. An element's table holds its style
//! properties beside its states' tables (and, for `taskbar`, the tables of
//! the elements under it), so the tables are walked by hand: each key is a
//! property, a state or a deeper path. Every unknown name is an error that
//! says where it is, and names vaht plans but lacks say so.

use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, Visitor};

use super::{BorderStyle, StylePatch, ThemeElement, ThemeLayers, WidgetState};
use crate::color::Color;
use crate::geometry::Insets;
use crate::text::{Font, FontWeight, Justify, TextEffect};
use crate::texture::{Bevel, GradientDirection, Texture};

/// Style properties of 02 §10.1 that vaht does not have yet.
const PLANNED_PROPERTIES: [&str; 5] = [
    "corner_radius",
    "corners",
    "alpha",
    "submenu_indicator",
    "interlaced",
];

/// States of 02 §10.1 that vaht does not have yet.
const PLANNED_STATES: [&str; 3] = ["hovered", "active", "disabled"];

/// Elements of 02 §10.1 that vaht does not have yet.
const PLANNED_ELEMENTS: [&str; 7] = [
    "taskbar.tray",
    "menu.title",
    "menu.frame",
    "menu.item",
    "tooltip",
    "snap-picker",
    "wireframe",
];

impl ThemeLayers {
    /// Reads a theme file's text.
    ///
    /// # Errors
    ///
    /// Returns an error naming the first problem and where it is.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
}

impl<'de> serde::Deserialize<'de> for ThemeLayers {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut layers = Self::default();
        deserializer.deserialize_map(Table {
            path: String::new(),
            layers: &mut layers,
        })?;
        Ok(layers)
    }
}

/// One table of a theme file, at `path` (empty for the file itself), whose
/// contents go into `layers`.
struct Table<'layers> {
    /// The dotted path of the table.
    path: String,
    /// Where what it says goes.
    layers: &'layers mut ThemeLayers,
}

impl Table<'_> {
    /// Returns the path of `key` under this table.
    fn child(&self, key: &str) -> String {
        if self.path.is_empty() {
            key.to_owned()
        } else {
            format!("{}.{key}", self.path)
        }
    }

    /// Returns whether `path` names an element or leads to one.
    fn leads_to_element(path: &str) -> bool {
        <ThemeElement as strum::IntoEnumIterator>::iter().any(|element| {
            element.path() == path
                || element
                    .path()
                    .strip_prefix(path)
                    .is_some_and(|rest| rest.starts_with('.'))
        })
    }
}

impl<'de> Visitor<'de> for Table<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a table of styles")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let element = ThemeElement::at_path(&self.path);
        while let Some(key) = map.next_key::<String>()? {
            if self.path.is_empty() && key == "default" {
                self.layers.default = map.next_value()?;
                continue;
            }
            if self.path.is_empty() && key == "state" {
                map.next_value_seed(States {
                    layers: &mut *self.layers,
                })?;
                continue;
            }
            if let Some(element) = element {
                let patch = self.layers.by_element.entry(element).or_default();
                if patch.read_property(&key, &mut map)? {
                    continue;
                }
                match state_named(&key) {
                    Ok(Some(state)) => {
                        let patch: StylePatch = map.next_value()?;
                        self.layers.by_element_state.insert((element, state), patch);
                        continue;
                    }
                    Ok(None) => {}
                    Err(reason) => return Err(refuse(&mut map, reason)),
                }
            }
            let path = self.child(&key);
            if PLANNED_ELEMENTS.contains(&path.as_str()) {
                return Err(refuse(
                    &mut map,
                    format!("`{path}` is not available in vaht yet"),
                ));
            }
            if !Self::leads_to_element(&path) {
                let reason = match element {
                    Some(_) => format!(
                        "`{key}` in [{}] is no style property, state or element",
                        self.path
                    ),
                    None => format!("`{path}` is no element of a theme"),
                };
                return Err(refuse(&mut map, reason));
            }
            map.next_value_seed(Table {
                path,
                layers: &mut *self.layers,
            })?;
        }
        Ok(())
    }
}

impl<'de> DeserializeSeed<'de> for Table<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}

/// The `[state]` table: one style per state.
struct States<'layers> {
    /// Where the states' styles go.
    layers: &'layers mut ThemeLayers,
}

impl<'de> Visitor<'de> for States<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a table of styles by state")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            let state = match state_named(&key) {
                Ok(Some(state)) => state,
                Ok(None) => {
                    let reason = format!(
                        "`{key}` is no state; the states are focused, unfocused and pressed"
                    );
                    return Err(refuse(&mut map, reason));
                }
                Err(reason) => return Err(refuse(&mut map, reason)),
            };
            let patch: StylePatch = map.next_value()?;
            self.layers.by_state.insert(state, patch);
        }
        Ok(())
    }
}

impl<'de> DeserializeSeed<'de> for States<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}

/// Returns the state `name` names, `None` if it names none, or why it is
/// refused: it names one vaht does not have yet.
fn state_named(name: &str) -> Result<Option<WidgetState>, String> {
    if PLANNED_STATES.contains(&name) {
        return Err(format!("the `{name}` state is not available in vaht yet"));
    }
    Ok(WidgetState::named(name))
}

/// Refuses the value of the key `map` is at, for `reason`: the error is
/// raised while that value is read, so it points at the value's own place
/// in the file rather than the table around it.
fn refuse<'de, A: MapAccess<'de>>(map: &mut A, reason: String) -> A::Error {
    match map.next_value_seed(Refusal(reason)) {
        Err(error) => error,
        Ok(never) => match never {},
    }
}

/// A value refused for its reason, whatever it holds.
struct Refusal(String);

impl<'de> DeserializeSeed<'de> for Refusal {
    type Value = std::convert::Infallible;

    fn deserialize<D: Deserializer<'de>>(self, _: D) -> Result<Self::Value, D::Error> {
        Err(de::Error::custom(self.0))
    }
}

impl StylePatch {
    /// Reads the property `key` from `map` into this patch; returns whether
    /// `key` is a property at all.
    fn read_property<'de, A: MapAccess<'de>>(
        &mut self,
        key: &str,
        map: &mut A,
    ) -> Result<bool, A::Error> {
        match key {
            "texture" => self.texture = Some(map.next_value::<TextureValue>()?.0),
            "font" => self.font = Some(map.next_value::<FontFile>()?.into_font()),
            "text_color" => self.text_color = Some(map.next_value::<ColorText>()?.0),
            "text_effect" => self.text_effect = Some(map.next_value::<TextEffectFile>()?.0),
            "justify" => self.justify = Some(map.next_value::<JustifyFile>()?.into_justify()),
            "glyph_color" => self.glyph_color = Some(map.next_value::<ColorText>()?.0),
            "padding" => {
                let [top, right, bottom, left] = map.next_value::<[u32; 4]>()?;
                self.padding = Some(Insets {
                    top,
                    right,
                    bottom,
                    left,
                });
            }
            "border" => self.border = Some(map.next_value::<BorderFile>()?.into_border()),
            "height" => self.height = Some(map.next_value()?),
            planned if PLANNED_PROPERTIES.contains(&planned) => {
                return Err(refuse(
                    map,
                    format!("the `{planned}` property is not available in vaht yet"),
                ));
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

impl<'de> serde::Deserialize<'de> for StylePatch {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads a table of style properties, and nothing else.
        struct PatchVisitor;

        impl<'de> Visitor<'de> for PatchVisitor {
            type Value = StylePatch;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a table of style properties")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<StylePatch, A::Error> {
                let mut patch = StylePatch::default();
                while let Some(key) = map.next_key::<String>()? {
                    if !patch.read_property(&key, &mut map)? {
                        return Err(de::Error::custom(format!("`{key}` is no style property")));
                    }
                }
                Ok(patch)
            }
        }

        deserializer.deserialize_map(PatchVisitor)
    }
}

impl<'de> serde::Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(de::Error::custom)
    }
}

/// A color as written: `#rrggbb`, `#rrggbbaa` or an X11 name.
#[derive(serde::Deserialize)]
#[serde(transparent)]
struct ColorText(Color);

/// A texture, checked while it is read so a mistake is located at it.
#[derive(serde::Deserialize)]
#[serde(try_from = "TextureFile")]
struct TextureValue(Texture);

impl TryFrom<TextureFile> for TextureValue {
    type Error = &'static str;

    fn try_from(file: TextureFile) -> Result<Self, Self::Error> {
        file.texture().map(Self)
    }
}

/// A texture as written: `{ solid = color }` or `{ gradient = direction,
/// from, to }`, either with a `bevel`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TextureFile {
    /// One color.
    solid: Option<Color>,
    /// The direction of a gradient.
    gradient: Option<GradientName>,
    /// A gradient's first color.
    from: Option<Color>,
    /// A gradient's last color.
    to: Option<Color>,
    /// The edge shading.
    #[serde(default)]
    bevel: BevelName,
}

impl TextureFile {
    /// Returns the texture written, or why it is not one.
    fn texture(self) -> Result<Texture, &'static str> {
        let bevel = self.bevel.into_bevel();
        match (self.solid, self.gradient, self.from, self.to) {
            (Some(color), None, None, None) => Ok(Texture::solid(color).with_bevel(bevel)),
            (None, Some(direction), Some(from), Some(to)) => {
                Ok(Texture::gradient(direction.into_direction(), from, to).with_bevel(bevel))
            }
            _ => Err(
                "a texture is { solid = color } or { gradient = direction, from = color, \
                 to = color }, either with a bevel",
            ),
        }
    }
}

/// A gradient's direction as written.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum GradientName {
    /// Top to bottom.
    Vertical,
    /// Left to right.
    Horizontal,
    /// Top-left to bottom-right.
    Diagonal,
    /// Top-right to bottom-left.
    CrossDiagonal,
}

impl GradientName {
    /// Returns the direction.
    const fn into_direction(self) -> GradientDirection {
        match self {
            Self::Vertical => GradientDirection::Vertical,
            Self::Horizontal => GradientDirection::Horizontal,
            Self::Diagonal => GradientDirection::Diagonal,
            Self::CrossDiagonal => GradientDirection::CrossDiagonal,
        }
    }
}

/// A bevel as written.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
enum BevelName {
    /// None.
    #[default]
    Flat,
    /// Raised.
    Raised,
    /// Sunken.
    Sunken,
}

impl BevelName {
    /// Returns the bevel.
    const fn into_bevel(self) -> Bevel {
        match self {
            Self::Flat => Bevel::Flat,
            Self::Raised => Bevel::Raised,
            Self::Sunken => Bevel::Sunken,
        }
    }
}

/// A font as written: `{ family, size, weight, style }`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FontFile {
    /// The family name.
    family: String,
    /// The size in points.
    size: u16,
    /// `normal` or `bold`.
    #[serde(default)]
    weight: WeightName,
    /// `normal` or `italic`.
    #[serde(default)]
    style: SlantName,
}

impl FontFile {
    /// Returns the font.
    fn into_font(self) -> Font {
        Font {
            family: self.family,
            size_points: self.size,
            weight: match self.weight {
                WeightName::Normal => FontWeight::Normal,
                WeightName::Bold => FontWeight::Bold,
            },
            is_italic: matches!(self.style, SlantName::Italic),
        }
    }
}

/// A font weight as written.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
enum WeightName {
    /// Regular.
    #[default]
    Normal,
    /// Heavier.
    Bold,
}

/// A font slant as written.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
enum SlantName {
    /// Upright.
    #[default]
    Normal,
    /// Slanted.
    Italic,
}

/// A justification as written.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum JustifyFile {
    /// Left.
    Left,
    /// Center.
    Center,
    /// Right.
    Right,
}

impl JustifyFile {
    /// Returns the justification.
    const fn into_justify(self) -> Justify {
        match self {
            Self::Left => Justify::Left,
            Self::Center => Justify::Center,
            Self::Right => Justify::Right,
        }
    }
}

/// A border as written: `{ width, color }`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BorderFile {
    /// Its width in pixels.
    width: u32,
    /// Its color.
    color: Color,
}

impl BorderFile {
    /// Returns the border.
    const fn into_border(self) -> BorderStyle {
        BorderStyle {
            width: self.width,
            color: self.color,
        }
    }
}

/// A text effect as written: `"none"`, `{ halo = color }`, or
/// `{ shadow = color, offset = [x, y] }`.
struct TextEffectFile(TextEffect);

impl<'de> serde::Deserialize<'de> for TextEffectFile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The table forms.
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct EffectTable {
            /// A halo's color.
            halo: Option<Color>,
            /// A shadow's color.
            shadow: Option<Color>,
            /// A shadow's offset, right and down.
            offset: Option<[i8; 2]>,
        }

        /// Reads either form.
        struct EffectVisitor;

        impl<'de> Visitor<'de> for EffectVisitor {
            type Value = TextEffectFile;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .write_str("\"none\", { halo = color } or { shadow = color, offset = [x, y] }")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<TextEffectFile, E> {
                if text == "none" {
                    Ok(TextEffectFile(TextEffect::None))
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(text), &self))
                }
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<TextEffectFile, A::Error> {
                let table = <EffectTable as serde::Deserialize>::deserialize(
                    de::value::MapAccessDeserializer::new(map),
                )?;
                match (table.halo, table.shadow, table.offset) {
                    (Some(color), None, None) => Ok(TextEffectFile(TextEffect::Halo(color))),
                    (None, Some(color), offset) => {
                        let [offset_x, offset_y] = offset.unwrap_or([1, 1]);
                        Ok(TextEffectFile(TextEffect::Shadow {
                            color,
                            offset_x,
                            offset_y,
                        }))
                    }
                    _ => Err(de::Error::invalid_value(de::Unexpected::Map, &self)),
                }
            }
        }

        deserializer.deserialize_any(EffectVisitor)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn the_built_in_theme_reads() {
        let layers = ThemeLayers::from_toml(crate::Theme::BUILT_IN);
        assert!(layers.is_ok(), "{layers:?}");
    }

    #[test]
    fn tables_become_the_four_layers() {
        let layers = ThemeLayers::from_toml(
            "[default]\ntext_color = \"red\"\n\
             [state.unfocused]\ntext_color = \"grey55\"\n\
             [window.label]\njustify = \"left\"\n\
             [window.label.pressed]\ntext_color = \"#000000\"\n\
             [taskbar]\nheight = 24\n\
             [taskbar.window-list.entry.focused]\ntexture = { solid = \"grey55\", bevel = \"raised\" }\n",
        )
        .expect("a valid theme");
        assert_eq!(layers.default.text_color, Some(Color::rgb(255, 0, 0)));
        assert_eq!(
            layers.by_state[&WidgetState::Unfocused].text_color,
            Some(Color::rgb(140, 140, 140))
        );
        assert_eq!(
            layers.by_element[&ThemeElement::Label].justify,
            Some(Justify::Left)
        );
        assert_eq!(
            layers.by_element_state[&(ThemeElement::Label, WidgetState::Pressed)].text_color,
            Some(Color::BLACK)
        );
        assert_eq!(layers.by_element[&ThemeElement::Taskbar].height, Some(24));
        assert_eq!(
            layers.by_element_state[&(ThemeElement::WindowListEntry, WidgetState::Focused)].texture,
            Some(Texture::solid(Color::rgb(140, 140, 140)).with_bevel(Bevel::Raised))
        );
    }

    #[rstest]
    #[case::unknown_element("[window.lable]\nheight = 1\n", "`window.lable` is no element", 1)]
    #[case::unknown_property("[window.label]\ncolour = \"red\"\n", "`colour` in [window.label]", 2)]
    #[case::planned_property("[window.label]\ncorner_radius = 4\n", "not available in vaht yet", 2)]
    #[case::planned_state(
        "[state.hovered]\ntext_color = \"red\"\n",
        "not available in vaht yet",
        1
    )]
    #[case::planned_element("[tooltip]\nheight = 3\n", "not available in vaht yet", 1)]
    #[case::unknown_state("[state.sleepy]\ntext_color = \"red\"\n", "is no state", 1)]
    #[case::bad_color("[default]\ntext_color = \"blurple\"\n", "not a color", 2)]
    #[case::half_gradient(
        "[default]\ntexture = { gradient = \"vertical\", from = \"red\" }\n",
        "a texture is",
        2
    )]
    #[case::property_outside_an_element(
        "[window]\nheight = 3\n",
        "`window.height` is no element",
        2
    )]
    fn mistakes_are_explained_where_they_are(
        #[case] text: &str,
        #[case] expected: &str,
        #[case] line: usize,
    ) {
        let error = ThemeLayers::from_toml(text).expect_err(text);
        assert!(error.message().contains(expected), "{error}");
        let start = error.span().expect("the problem is located").start;
        let found = text[..start].matches('\n').count() + 1;
        assert_eq!(found, line, "located: {error}");
    }
}

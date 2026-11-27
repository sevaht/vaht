//! Reading style files (02 §10.1): TOML tables into [`StyleLayers`].
//!
//! A style file's tables are its layers: `[default]`, `[state.<state>]`,
//! `[<element>]` and `[<element>.<state>]`, where an element is a dotted
//! path such as `window.titlebar`. An element's table holds its style
//! properties beside its states' tables (and, for `taskbar`, the tables of
//! the elements under it), so the tables are walked by hand: each key is a
//! property, a state or a deeper path. Every unknown name is an error that
//! says where it is, and names vaht plans but lacks say so.
//!
//! Rounded corners (`corner_radius`, `corners`) shape a window's frame or a
//! menu, so only `[window.border]` and `[menu.frame]`, their outlines, take
//! them.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, Visitor};

use super::options::{self, NoLibrary, Selection, Variables};
use super::{
    BorderPatch, Bullet, CheckMark, ElementStylePatch, GlyphImages, StyleElement, StyleLayers,
    WidgetState,
};
use crate::color::Color;
use crate::corner::{Corner, CornerSet};
use crate::image::Image;
use crate::length::{Length, Sides};
use crate::point_size::PointSize;
use crate::text::{Font, FontWeight, Justify, TextEffect};
use crate::texture::{Bevel, GradientDirection, ImageFit, Texture};
use crate::widget::{Glyph, MarkSide};

/// Style properties of 02 §10.1 that vaht does not have yet.
const PLANNED_PROPERTIES: [&str; 2] = ["submenu_indicator", "interlaced"];

/// The elements drawn whole with an `alpha`: the taskbar, a menu and a
/// tooltip.
const TRANSLUCENT_ELEMENTS: [StyleElement; 3] = [
    StyleElement::Taskbar,
    StyleElement::MenuFrame,
    StyleElement::Tooltip,
];

/// The properties only the element that may be rounded takes.
const CORNER_PROPERTIES: [&str; 2] = ["corner_radius", "corners"];

/// The elements whose corners may be rounded: a window frame's outline and
/// a menu's.
const ROUNDED_ELEMENTS: [StyleElement; 3] = [
    StyleElement::Border,
    StyleElement::MenuFrame,
    StyleElement::Tooltip,
];

/// States of 02 §10.1 that vaht does not have yet.
const PLANNED_STATES: [&str; 1] = ["active"];

/// Elements of 02 §10.1 that vaht does not have yet.
const PLANNED_ELEMENTS: [&str; 2] = ["taskbar.tray", "snap-picker"];

impl StyleLayers {
    /// Reads a style file's text.
    ///
    /// # Errors
    ///
    /// Returns an error naming the first problem and where it is.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        Self::from_toml_in(text, None)
    }

    /// Reads a style file's text; the images it names are files relative to
    /// `directory` (none: it names none).
    ///
    /// # Errors
    ///
    /// Returns an error naming the first problem and where it is, including
    /// an image that cannot be read.
    pub fn from_toml_in(text: &str, directory: Option<&Path>) -> Result<Self, toml::de::Error> {
        Self::from_toml_selecting(
            text,
            &Selection {
                directory,
                chosen: &BTreeMap::new(),
                library: &NoLibrary,
            },
        )
    }

    /// Reads a style file's text, with the choices of its options that
    /// `selection` makes (02 §10.5); images and choice files it names are
    /// beside it, in `selection.directory`.
    ///
    /// # Errors
    ///
    /// Returns an error naming the first problem and where it is. A choice
    /// that cannot be used is no error: the option's default is used, and
    /// [`StyleLayers::problems`] says so.
    pub fn from_toml_selecting(
        text: &str,
        selection: &Selection<'_>,
    ) -> Result<Self, toml::de::Error> {
        let resolved = options::resolve(text, selection)?;
        let mut layers = Self::default();
        let images = Context {
            directory: selection.directory,
            variables: &resolved.variables,
        };
        let deserializer = toml::de::Deserializer::parse(text)?;
        deserializer.deserialize_map(Table {
            path: String::new(),
            layers: &mut layers,
            images,
        })?;
        layers.options = resolved.options;
        layers.problems = resolved.problems;
        Ok(layers)
    }
}

/// What reading a value needs to know: where the images a style file names
/// are (files in `directory`), and the variables its options give.
#[derive(Clone, Copy)]
struct Context<'context> {
    /// The directory the names are relative to; none where there are no
    /// files (a style read from text alone).
    directory: Option<&'context Path>,
    /// The variables a value may refer to.
    variables: &'context Variables,
}

impl Context<'_> {
    /// Reads the next value of `map` as a `T`, after the variables it refers
    /// to are put in.
    fn value<'de, A: MapAccess<'de>, T: de::DeserializeOwned>(
        self,
        map: &mut A,
    ) -> Result<T, A::Error> {
        self.value_with(map, std::marker::PhantomData::<T>)
    }

    /// Reads the next value of `map` with `seed`, after the variables it
    /// refers to are put in.
    fn value_with<'de, A: MapAccess<'de>, S: DeserializeSeed<'de>>(
        self,
        map: &mut A,
        seed: S,
    ) -> Result<S::Value, A::Error> {
        map.next_value_seed(Substituting {
            variables: self.variables,
            seed,
        })
    }

    /// Reads the image file `name`, relative to the style's directory.
    fn load(self, name: &str) -> Result<Arc<Image>, String> {
        let directory = self
            .directory
            .ok_or_else(|| format!("`{name}`: this style is not a file, so it has no images"))?;
        let path = Path::new(name);
        // A style's images are beside it, not anywhere on the disk.
        if path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(format!(
                "`{name}`: an image is a path inside the style's directory"
            ));
        }
        let text = std::fs::read_to_string(directory.join(path))
            .map_err(|error| format!("`{name}`: {error}"))?;
        Image::from_xpm(&text)
            .map(Arc::new)
            .map_err(|error| format!("`{name}`: {error}"))
    }
}

/// One table of a style file, at `path` (empty for the file itself), whose
/// contents go into `layers`.
struct Table<'layers> {
    /// The dotted path of the table.
    path: String,
    /// Where what it says goes.
    layers: &'layers mut StyleLayers,
    /// Where the images it names are.
    images: Context<'layers>,
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
        <StyleElement as strum::IntoEnumIterator>::iter().any(|element| {
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
        let element = StyleElement::at_path(&self.path);
        while let Some(key) = map.next_key::<String>()? {
            if self.path.is_empty() && key == "options" {
                // Declarations, read before the layers (options.rs).
                map.next_value::<de::IgnoredAny>()?;
                continue;
            }
            if self.path.is_empty() && key == "default" {
                self.layers.default = map.next_value_seed(PatchSeed(self.images))?;
                continue;
            }
            if self.path.is_empty() && key == "state" {
                map.next_value_seed(States {
                    layers: &mut *self.layers,
                    images: self.images,
                })?;
                continue;
            }
            if let Some(element) = element {
                let patch = self.layers.by_element.entry(element).or_default();
                if patch.read_property(&key, &mut map, Some(element), self.images)? {
                    continue;
                }
                match state_named(&key) {
                    Ok(Some(state)) => {
                        let patch = map.next_value_seed(PatchSeed(self.images))?;
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
                    None => format!("`{path}` is no element of a style"),
                };
                return Err(refuse(&mut map, reason));
            }
            map.next_value_seed(Table {
                path,
                layers: &mut *self.layers,
                images: self.images,
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
    layers: &'layers mut StyleLayers,
    /// Where the images they name are.
    images: Context<'layers>,
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
            let patch = map.next_value_seed(PatchSeed(self.images))?;
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

/// A value read as TOML, with the variables it refers to put in, and then
/// read by `seed`.
struct Substituting<'variables, S> {
    /// The variables a value may refer to.
    variables: &'variables Variables,
    /// How the value, once complete, is read.
    seed: S,
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for Substituting<'_, S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<S::Value, D::Error> {
        let value = <toml::Value as serde::Deserialize>::deserialize(deserializer)?;
        let value = self
            .variables
            .substitute(value)
            .map_err(de::Error::custom)?;
        self.seed
            .deserialize(value)
            .map_err(|error: toml::de::Error| de::Error::custom(error.message()))
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

impl ElementStylePatch {
    /// Reads the property `key` from `map` into this patch, for `element`
    /// alone (none: for several, as the default and states are); returns
    /// whether `key` is a property at all.
    fn read_property<'de, A: MapAccess<'de>>(
        &mut self,
        key: &str,
        map: &mut A,
        element: Option<StyleElement>,
        images: Context<'_>,
    ) -> Result<bool, A::Error> {
        if CORNER_PROPERTIES.contains(&key)
            && !element.is_some_and(|element| ROUNDED_ELEMENTS.contains(&element))
        {
            return Err(refuse(
                map,
                format!(
                    "`{key}` goes in [{}], [{}] or [{}] alone: only frames, menus and \
                     tooltips are rounded",
                    ROUNDED_ELEMENTS[0].path(),
                    ROUNDED_ELEMENTS[1].path(),
                    ROUNDED_ELEMENTS[2].path()
                ),
            ));
        }
        if key == "alpha" && !element.is_some_and(|element| TRANSLUCENT_ELEMENTS.contains(&element))
        {
            return Err(refuse(
                map,
                format!(
                    "`alpha` goes in [{}], [{}] or [{}] alone: the taskbar, menus and \
                     tooltips are drawn translucent whole",
                    TRANSLUCENT_ELEMENTS[0].path(),
                    TRANSLUCENT_ELEMENTS[1].path(),
                    TRANSLUCENT_ELEMENTS[2].path()
                ),
            ));
        }
        match key {
            "alpha" => self.alpha = Some(images.value(map)?),
            "texture" => self.texture = Some(images.value_with(map, TextureSeed(images))?),
            "font" => self.font = Some(images.value::<_, FontFile>(map)?.into_font()),
            "text_color" => self.text_color = Some(images.value::<_, ColorText>(map)?.0),
            "text_effect" => self.text_effect = Some(images.value::<_, TextEffectFile>(map)?.0),
            "justify" => self.justify = Some(images.value::<_, JustifyFile>(map)?.into_justify()),
            "glyph_color" => self.glyph_color = Some(images.value::<_, ColorText>(map)?.0),
            "restore_glyph" => self.restore_glyph = Some(images.value(map)?),
            "glyph_images" => {
                self.glyph_images = images.value_with(map, GlyphImagesSeed(images))?;
            }
            "padding" => self.padding = Some(images.value::<_, Sides>(map)?.0),
            "spacing" => self.spacing = Some(images.value(map)?),
            "border" => self.border = Some(images.value::<_, BorderFile>(map)?.into_border()),
            "height" => self.height = Some(images.value(map)?),
            "corner_radius" => self.corner_radius = Some(images.value(map)?),
            "corners" => self.rounded_corners = Some(images.value::<_, CornersFile>(map)?.0),
            "bullet" => self.bullet = Some(images.value::<_, BulletName>(map)?.into_bullet()),
            "check" => self.check = Some(images.value::<_, CheckName>(map)?.into_check()),
            "bullet_position" => {
                self.bullet_side = Some(images.value::<_, SideName>(map)?.into_side());
            }
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

/// Reads a table of style properties into a patch.
struct PatchSeed<'directory>(Context<'directory>);

impl<'de> DeserializeSeed<'de> for PatchSeed<'_> {
    type Value = ElementStylePatch;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        /// Reads a table of style properties, and nothing else.
        struct PatchVisitor<'directory>(Context<'directory>);

        impl<'de> Visitor<'de> for PatchVisitor<'_> {
            type Value = ElementStylePatch;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a table of style properties")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<ElementStylePatch, A::Error> {
                let mut patch = ElementStylePatch::default();
                while let Some(key) = map.next_key::<String>()? {
                    if !patch.read_property(&key, &mut map, None, self.0)? {
                        return Err(de::Error::custom(format!("`{key}` is no style property")));
                    }
                }
                Ok(patch)
            }
        }

        deserializer.deserialize_map(PatchVisitor(self.0))
    }
}

impl<'de> serde::Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(de::Error::custom)
    }
}

/// A table of button pictures: each glyph's name and its image file.
struct GlyphImagesSeed<'directory>(Context<'directory>);

impl<'de> DeserializeSeed<'de> for GlyphImagesSeed<'_> {
    type Value = GlyphImages;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<GlyphImages, D::Error> {
        /// Reads the table.
        struct GlyphImagesVisitor<'directory>(Context<'directory>);

        impl<'de> Visitor<'de> for GlyphImagesVisitor<'_> {
            type Value = GlyphImages;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "a table of image files by button: close, maximize, restore, minimize, \
                     shade, unshade, stick, stuck, menu, submenu or check",
                )
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<GlyphImages, A::Error> {
                let mut images = GlyphImages::default();
                while let Some(glyph) = map.next_key::<GlyphName>()? {
                    let name = map.next_value::<String>()?;
                    let image = self.0.load(&name).map_err(de::Error::custom)?;
                    for glyph in glyph.glyphs() {
                        images = images.with(*glyph, Arc::clone(&image));
                    }
                }
                Ok(images)
            }
        }

        deserializer.deserialize_map(GlyphImagesVisitor(self.0))
    }
}

/// How an image covers an element, as named in a style file.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FitName {
    /// Scaled to fit.
    Stretch,
    /// Repeated at its own size.
    Tile,
}

impl FitName {
    /// Returns the fit.
    const fn into_fit(self) -> ImageFit {
        match self {
            Self::Stretch => ImageFit::Stretch,
            Self::Tile => ImageFit::Tile,
        }
    }
}

/// A titlebar button's picture as named in a style file.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum GlyphName {
    /// An X.
    Close,
    /// A box.
    Maximize,
    /// Two boxes.
    Restore,
    /// A low bar.
    Minimize,
    /// A high bar.
    Shade,
    /// A high bar over a lower one.
    Unshade,
    /// A dot.
    Stick,
    /// A dot, pinned.
    Stuck,
    /// Three bars.
    Menu,
    /// A submenu item's bullet, whatever its shape.
    Submenu,
    /// A chosen menu item's check.
    Check,
}

impl GlyphName {
    /// Returns the glyphs it names.
    const fn glyphs(self) -> &'static [Glyph] {
        match self {
            Self::Close => &[Glyph::Close],
            Self::Maximize => &[Glyph::Maximize],
            Self::Restore => &[Glyph::Restore],
            Self::Minimize => &[Glyph::Minimize],
            Self::Shade => &[Glyph::Shade],
            Self::Unshade => &[Glyph::Unshade],
            Self::Stick => &[Glyph::Stick],
            Self::Stuck => &[Glyph::Stuck],
            Self::Menu => &[Glyph::Menu],
            Self::Submenu => &[
                Glyph::Submenu,
                Glyph::BulletRight,
                Glyph::BulletLeft,
                Glyph::BulletSquare,
                Glyph::BulletDiamond,
                Glyph::BulletChevron,
                Glyph::BulletDoubleChevron,
            ],
            Self::Check => &[Glyph::Check, Glyph::Tick],
        }
    }
}

/// A color as written: `#rrggbb`, `#rrggbbaa` or an X11 name.
#[derive(serde::Deserialize)]
#[serde(transparent)]
struct ColorText(Color);

/// A texture, checked while it is read so a mistake is located at it:
/// `"parent"`, or a table ([`TextureFile`]).
struct TextureSeed<'directory>(Context<'directory>);

impl<'de> DeserializeSeed<'de> for TextureSeed<'_> {
    type Value = Texture;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Texture, D::Error> {
        /// Reads either form.
        struct TextureVisitor<'directory>(Context<'directory>);

        impl<'de> Visitor<'de> for TextureVisitor<'_> {
            type Value = Texture;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "\"parent\", { solid = color }, { gradient = direction, from = color, \
                     to = color } or { image = \"file.xpm\", fit = stretch|tile }",
                )
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Texture, E> {
                if text == "parent" {
                    Ok(Texture::parent())
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(text), &self))
                }
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Texture, A::Error> {
                let file = <TextureFile as serde::Deserialize>::deserialize(
                    de::value::MapAccessDeserializer::new(map),
                )?;
                file.texture(self.0).map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_any(TextureVisitor(self.0))
    }
}

/// A texture as written: `{ solid = color }`, `{ gradient = direction, from,
/// to }` (either with a `bevel`), or `{ image = "file.xpm" }`.
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
    /// An image file, relative to the style's directory.
    image: Option<String>,
    /// How the image covers the element (stretched, if not said).
    fit: Option<FitName>,
    /// The edge shading.
    #[serde(default)]
    bevel: BevelName,
}

impl TextureFile {
    /// Returns the texture written, or why it is not one.
    fn texture(self, images: Context<'_>) -> Result<Texture, String> {
        let bevel = self.bevel.into_bevel();
        if self.fit.is_some() && self.image.is_none() {
            return Err("`fit` is how an image covers an element: it goes with `image`".to_owned());
        }
        let fit = self.fit.map_or(ImageFit::Stretch, FitName::into_fit);
        match (self.solid, self.gradient, self.from, self.to, self.image) {
            (Some(color), None, None, None, None) => Ok(Texture::solid(color).with_bevel(bevel)),
            (None, Some(direction), Some(from), Some(to), None) => {
                Ok(Texture::gradient(direction.into_direction(), from, to).with_bevel(bevel))
            }
            (None, None, None, None, Some(name)) => Ok(Texture::image(images.load(&name)?, fit)),
            _ => Err(
                "a texture is { solid = color }, { gradient = direction, from = color, \
                      to = color } (either with a bevel) or { image = \"file.xpm\", fit = stretch|tile }"
                    .to_owned(),
            ),
        }
    }
}

/// A submenu bullet as written.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum BulletName {
    /// None.
    Empty,
    /// A triangle.
    Triangle,
    /// A square's outline.
    Square,
    /// A diamond.
    Diamond,
    /// A chevron.
    Chevron,
    /// Two chevrons.
    DoubleChevron,
}

/// A chosen item's mark as written.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CheckName {
    /// A filled square.
    Box,
    /// A tick.
    Tick,
}

impl CheckName {
    /// Returns the mark.
    const fn into_check(self) -> CheckMark {
        match self {
            Self::Box => CheckMark::Box,
            Self::Tick => CheckMark::Tick,
        }
    }
}

impl BulletName {
    /// Returns the bullet.
    const fn into_bullet(self) -> Bullet {
        match self {
            Self::Empty => Bullet::Empty,
            Self::Triangle => Bullet::Triangle,
            Self::Square => Bullet::Square,
            Self::Diamond => Bullet::Diamond,
            Self::Chevron => Bullet::Chevron,
            Self::DoubleChevron => Bullet::DoubleChevron,
        }
    }
}

/// A side as written.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SideName {
    /// The left.
    Left,
    /// The right.
    Right,
}

impl SideName {
    /// Returns the side.
    const fn into_side(self) -> MarkSide {
        match self {
            Self::Left => MarkSide::Left,
            Self::Right => MarkSide::Right,
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
    /// The size in points: a number, or a sum of numbers.
    size: PointSize,
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
            size_points: self.size.points(),
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

/// Rounded corners as written: a list of names such as `"top-left"`.
struct CornersFile(CornerSet);

impl<'de> serde::Deserialize<'de> for CornersFile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(deserializer)?;
        names
            .iter()
            .map(|name| {
                Corner::named(name).ok_or_else(|| {
                    de::Error::custom(format!(
                        "`{name}` is no corner; the corners are top-left, top-right, \
                         bottom-left and bottom-right"
                    ))
                })
            })
            .collect::<Result<CornerSet, D::Error>>()
            .map(Self)
    }
}

/// A border as written: `{ width, color }`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BorderFile {
    /// Its width.
    width: Length,
    /// Its color.
    color: Color,
}

impl BorderFile {
    /// Returns the border.
    const fn into_border(self) -> BorderPatch {
        BorderPatch {
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
    use crate::Typesetter;
    use rstest::rstest;

    use super::*;

    #[test]
    fn the_built_in_style_reads() {
        let layers = StyleLayers::from_toml(crate::Style::BUILT_IN);
        assert!(layers.is_ok(), "{layers:?}");
    }

    #[test]
    fn the_frame_s_outline_takes_rounded_corners() {
        let layers = StyleLayers::from_toml(
            "[window.border]\ncorner_radius = 8\ncorners = [\"top-left\", \"top-right\"]\n",
        )
        .expect("a valid style");
        let style = crate::Style::resolve(&layers);
        assert_eq!(
            style.metrics().frame_corners,
            crate::CornerRadii::of(8, [Corner::TopLeft, Corner::TopRight])
        );
    }

    #[test]
    fn tables_become_the_four_layers() {
        let layers = StyleLayers::from_toml(
            "[default]\ntext_color = \"red\"\n\
             [state.unfocused]\ntext_color = \"grey55\"\n\
             [window.label]\njustify = \"left\"\n\
             [window.label.pressed]\ntext_color = \"#000000\"\n\
             [taskbar]\nheight = 24\n\
             [taskbar.window-list.entry.focused]\ntexture = { solid = \"grey55\", bevel = \"raised\" }\n",
        )
        .expect("a valid style");
        assert_eq!(layers.default.text_color, Some(Color::rgb(255, 0, 0)));
        assert_eq!(
            layers.by_state[&WidgetState::Unfocused].text_color,
            Some(Color::rgb(140, 140, 140))
        );
        assert_eq!(
            layers.by_element[&StyleElement::Label].justify,
            Some(Justify::Left)
        );
        assert_eq!(
            layers.by_element_state[&(StyleElement::Label, WidgetState::Pressed)].text_color,
            Some(Color::BLACK)
        );
        assert_eq!(
            layers.by_element[&StyleElement::Taskbar].height,
            Some(Length::pixels(24))
        );
        assert_eq!(
            layers.by_element_state[&(StyleElement::WindowListEntry, WidgetState::Focused)].texture,
            Some(Texture::solid(Color::rgb(140, 140, 140)).with_bevel(Bevel::Raised))
        );
    }

    #[rstest]
    #[case::unknown_element("[window.lable]\nheight = 1\n", "`window.lable` is no element", 1)]
    #[case::unknown_property("[window.label]\ncolour = \"red\"\n", "`colour` in [window.label]", 2)]
    #[case::planned_property("[window.label]\ninterlaced = true\n", "not available in vaht yet", 2)]
    #[case::alpha_elsewhere("[window.label]\nalpha = 4\n", "the taskbar, menus and tooltips", 2)]
    #[case::corners_off_the_frame(
        "[window.label]\ncorner_radius = 4\n",
        "goes in [window.border], [menu.frame] or [tooltip] alone",
        2
    )]
    #[case::corners_for_every_element(
        "[default]\ncorner_radius = 4\n",
        "goes in [window.border]",
        2
    )]
    #[case::unknown_corner(
        "[window.border]\ncorners = [\"top-left\", \"middle\"]\n",
        "`middle` is no corner",
        2
    )]
    #[case::planned_state(
        "[state.active]\ntext_color = \"red\"\n",
        "not available in vaht yet",
        1
    )]
    #[case::planned_element("[snap-picker]\nheight = 3\n", "not available in vaht yet", 1)]
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
        let error = StyleLayers::from_toml(text).expect_err(text);
        assert!(error.message().contains(expected), "{error}");
        let start = error.span().expect("the problem is located").start;
        let found = text[..start].matches('\n').count() + 1;
        assert_eq!(found, line, "located: {error}");
    }

    /// An XPM of one black pixel.
    const DOT: &str = "\"1 1 1 1\",\n\"a\tc #000000\",\n\"a\"";

    #[test]
    fn an_image_texture_is_an_image_read_from_beside_the_style() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(directory.path().join("dot.xpm"), DOT).expect("written");
        let layers = StyleLayers::from_toml_in(
            "[window.titlebar]\ntexture = { image = \"dot.xpm\" }\n",
            Some(directory.path()),
        )
        .expect("valid");
        let texture = layers
            .by_element
            .get(&StyleElement::Titlebar)
            .and_then(|patch| patch.texture.clone())
            .expect("a texture");
        assert!(matches!(
            texture.fill,
            crate::texture::Fill::Image {
                fit: ImageFit::Stretch,
                ..
            }
        ));
        let tiled = StyleLayers::from_toml_in(
            "[window.titlebar]\ntexture = { image = \"dot.xpm\", fit = \"tile\" }\n",
            Some(directory.path()),
        )
        .expect("valid");
        assert!(matches!(
            tiled.by_element[&StyleElement::Titlebar]
                .texture
                .as_ref()
                .map(|texture| &texture.fill),
            Some(crate::texture::Fill::Image {
                fit: ImageFit::Tile,
                ..
            })
        ));
        let misplaced = StyleLayers::from_toml(
            "[window.titlebar]\ntexture = { solid = \"red\", fit = \"tile\" }\n",
        )
        .expect_err("fit needs an image")
        .to_string();
        assert!(misplaced.contains("goes with `image`"), "{misplaced}");
    }

    #[test]
    fn an_image_cannot_leave_the_styles_directory_or_be_missing() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        for (name, expected) in [
            ("../escape.xpm", "inside the style's directory"),
            ("/etc/passwd", "inside the style's directory"),
            ("absent.xpm", "absent.xpm"),
        ] {
            let text = format!("[window.titlebar]\ntexture = {{ image = \"{name}\" }}\n");
            let error = StyleLayers::from_toml_in(&text, Some(directory.path()))
                .expect_err(name)
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
        let error = StyleLayers::from_toml("[window.titlebar]\ntexture = { image = \"a.xpm\" }\n")
            .expect_err("no directory")
            .to_string();
        assert!(error.contains("not a file"), "{error}");
    }

    /// The resolved style of `text` for the titlebar's button, focused.
    fn button(text: &str) -> crate::style::ElementStyle {
        let layers = StyleLayers::from_toml(text).expect("valid");
        crate::Style::resolve(&layers)
            .element_style(StyleElement::Button, WidgetState::Focused)
            .clone()
    }

    /// A style whose button takes a texture, a text color, a padding and a
    /// border from the variables of two options.
    const OPTIONAL_STYLE: &str = r#"
[options.scheme]
default = "graphite"
[options.scheme.graphite]
face = { gradient = "vertical", from = "grey30", to = "grey5" }
ink = "white"
[options.scheme.blue]
face = { solid = "navy" }

[options.size]
default = "medium"
[options.size.medium]
pad = 4
edge = { width = 1, color = "grey60" }
[options.size.large]
pad = 8

[window.button]
texture = "$face"
text_color = "$ink"
padding = "$pad"
spacing = "1lh + $pad"
border = "$edge"
"#;

    fn selecting(text: &str, chosen: &[(&str, &str)]) -> StyleLayers {
        let chosen = chosen
            .iter()
            .map(|(option, choice)| ((*option).to_owned(), (*choice).to_owned()))
            .collect();
        StyleLayers::from_toml_selecting(
            text,
            &Selection {
                directory: None,
                chosen: &chosen,
                library: &NoLibrary,
            },
        )
        .expect("valid")
    }

    fn button_of(layers: &StyleLayers) -> crate::style::ElementStyle {
        crate::Style::resolve(layers)
            .element_style(StyleElement::Button, WidgetState::Focused)
            .clone()
    }

    #[test]
    fn variables_stand_in_for_textures_colors_lengths_and_borders() {
        let defaults = button_of(&selecting(OPTIONAL_STYLE, &[]));
        assert_eq!(defaults.padding.top, 4);
        assert_eq!(defaults.border.width, 1);
        assert_eq!(defaults.text.color, Color::rgb(255, 255, 255));
        let chosen = button_of(&selecting(
            OPTIONAL_STYLE,
            &[("scheme", "blue"), ("size", "large")],
        ));
        assert_eq!(chosen.padding.top, 8, "the size's own choice");
        assert_eq!(
            chosen.border.width, 1,
            "the default's, which large leaves out"
        );
        assert_eq!(
            chosen.text.color,
            Color::rgb(255, 255, 255),
            "blue leaves ink out"
        );
        assert_ne!(chosen.texture, defaults.texture, "navy, not the gradient");
        // Text with a variable in it is a length like any other.
        assert_eq!(
            button_of(&selecting(OPTIONAL_STYLE, &[("size", "large")])).spacing,
            button_of(&selecting(OPTIONAL_STYLE, &[])).spacing + 4,
            "1lh + 8 is four more than 1lh + 4"
        );
    }

    #[test]
    fn the_options_a_style_declares_are_listed_with_the_choice_in_effect() {
        let layers = selecting(OPTIONAL_STYLE, &[("scheme", "blue")]);
        let listed: Vec<(&str, &str, &[String])> = layers
            .options
            .options
            .iter()
            .map(|option| {
                (
                    option.name.as_str(),
                    option.chosen.as_str(),
                    option.choices.as_slice(),
                )
            })
            .collect();
        assert_eq!(
            listed,
            [
                (
                    "scheme",
                    "blue",
                    &["graphite".to_owned(), "blue".to_owned()][..]
                ),
                (
                    "size",
                    "medium",
                    &["medium".to_owned(), "large".to_owned()][..]
                ),
            ]
        );
    }

    #[test]
    fn a_variable_the_style_does_not_define_is_an_error_at_its_line() {
        let error = StyleLayers::from_toml("[window.button]\npadding = \"$nowhere\"\n")
            .expect_err("no such variable")
            .to_string();
        assert!(error.contains("`$nowhere`"), "{error}");
        assert!(error.contains("line 2"), "{error}");
    }

    #[test]
    fn a_style_without_options_reads_as_before() {
        let layers = StyleLayers::from_toml("[window.button]\npadding = 3\n").expect("valid");
        assert_eq!(layers.options, options::StyleOptions::default());
        assert_eq!(layers.problems, Vec::<String>::new());
    }

    #[test]
    fn a_choice_that_is_not_there_is_noted_and_the_default_used() {
        let layers = selecting(OPTIONAL_STYLE, &[("size", "huge")]);
        assert_eq!(button_of(&layers).padding.top, 4);
        assert_eq!(layers.problems.len(), 1, "{:?}", layers.problems);
        assert!(layers.problems[0].contains("huge"), "{:?}", layers.problems);
    }

    #[test]
    fn one_length_pads_all_four_sides() {
        let sides = |padding: &str| {
            let style = button(&format!("[window.button]\npadding = {padding}\n"));
            [
                style.padding.top,
                style.padding.right,
                style.padding.bottom,
                style.padding.left,
            ]
        };
        assert_eq!(sides("5"), [5; 4]);
        assert_eq!(
            sides("\"1em\""),
            sides("[\"1em\", \"1em\", \"1em\", \"1em\"]")
        );
        assert_eq!(sides("[1, 2, 3, 4]"), [1, 2, 3, 4]);
        for wrong in ["[1, 2, 3]", "[1, 2, 3, 4, 5]", "-1", "\"wide\""] {
            assert!(
                StyleLayers::from_toml(&format!("[window.button]\npadding = {wrong}\n")).is_err(),
                "{wrong} is not padding"
            );
        }
    }

    #[test]
    fn a_length_in_em_is_the_elements_font_size_in_pixels() {
        // 12 points is 16 pixels at 96 dots an inch.
        let style = button(
            "[default]\nfont = { family = \"sans\", size = 12 }\n\
             [window.button]\npadding = [\"0.5em\", 4, \"1em\", \"12px\"]\n\
             spacing = \"0.25em\"\nheight = \"2em\"\n\
             border = { width = \"0.125em\", color = \"red\" }\n",
        );
        assert_eq!(
            (
                style.padding.top,
                style.padding.right,
                style.padding.bottom,
                style.padding.left
            ),
            (8, 4, 16, 12)
        );
        assert_eq!(style.spacing, 4);
        assert_eq!(style.height, Some(32));
        assert_eq!(style.border.width, 2);
        // A frame's corners may be rounded by an em too.
        let layers = StyleLayers::from_toml(
            "[default]\nfont = { family = \"sans\", size = 12 }\n\
             [window.border]\ncorner_radius = \"0.5em\"\n",
        )
        .expect("valid");
        let frame = crate::Style::resolve(&layers)
            .element_style(StyleElement::Border, WidgetState::Focused)
            .clone();
        assert_eq!(frame.corner_radius, 8);
    }

    #[test]
    fn an_em_follows_the_font_a_later_layer_gives_the_element() {
        // The padding is set in one layer and the font in another: the font
        // the element ends up with decides what an em is.
        let style = button(
            "[default]\nfont = { family = \"sans\", size = 12 }\n\
             [window.button]\npadding = [\"1em\", \"1em\", \"1em\", \"1em\"]\n\
             font = { family = \"sans\", size = 6 }\n",
        );
        assert_eq!(
            style.padding.top, 8,
            "6 points is 8 pixels, not the default's 16"
        );
    }

    #[test]
    fn a_bad_length_says_what_a_length_is_and_where() {
        let error = StyleLayers::from_toml("[window.button]\nspacing = \"3 ems\"\n")
            .expect_err("not a length")
            .to_string();
        assert!(error.contains("a length"), "{error}");
        assert!(error.contains("spacing"), "{error}");
    }

    #[test]
    fn a_length_in_lines_is_the_height_of_the_elements_font_plus_what_is_added() {
        let font = Font {
            family: "sans".to_owned(),
            size_points: 12,
            weight: FontWeight::Normal,
            is_italic: false,
        };
        let line = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH).font_height(&font);
        // Fluxbox's rule for a titlebar: the font's height, 2, and a bevel of
        // 3 at both ends.
        let layers = StyleLayers::from_toml(
            "[window.titlebar]\nfont = { family = \"sans\", size = 12 }\n\
             height = \"1lh + 2 + 6\"\npadding = [3, 3, 3, 3]\n\
             [window.titlebar.hovered]\nheight = \"1lh + 4\"\n",
        )
        .expect("valid");
        let style = crate::Style::resolve(&layers);
        let titlebar = style.element_style(StyleElement::Titlebar, WidgetState::Focused);
        assert_eq!(titlebar.height, Some(line + 8));
        let hovered =
            style.element_style_under_pointer(StyleElement::Titlebar, WidgetState::Focused, true);
        assert_eq!(
            hovered.height,
            Some(line + 4),
            "a hovered look measures too"
        );
    }
}

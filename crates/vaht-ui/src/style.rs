//! What every themable thing looks like in every state.
//!
//! A [`Style`] answers one question: the [`ElementStyle`] of a [`StyleElement`] in
//! a [`WidgetState`]. It is resolved once from [`StyleLayers`], the four
//! layers a style file writes (02 §10.1), each overriding the one before:
//!
//! 1. the default, for everything;
//! 2. one state, for every element;
//! 3. one element, in every state;
//! 4. one element in one state.
//!
//! These replace Fluxbox's wildcard resource matching. Drawing never cascades:
//! it looks the resolved style up. Style files are TOML (`style/file.rs`
//! reads them); the default style is one too, `data/styles/vaht.toml`, built
//! in and installed, after the `nacitar_fbstyle` Fluxbox style.

mod file;

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::color::Color;
use crate::corner::{CornerRadii, CornerSet};
use crate::geometry::Insets;
use crate::image::Image;
use crate::text::{Font, FontWeight, Justify, TextEffect, TextStyle};
use crate::texture::Texture;
use crate::widget::{Glyph, MarkSide};

/// Something a style describes the look of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter)]
pub enum StyleElement {
    /// The background of a window's titlebar.
    Titlebar,
    /// The area showing a window's title.
    Label,
    /// A titlebar button (close, maximize…).
    Button,
    /// The strip along a window's bottom edge.
    Handle,
    /// The resize grips at the handle's ends.
    Grip,
    /// The line around a window.
    Border,
    /// The taskbar's background.
    Taskbar,
    /// A taskbar button, such as the next-workspace arrow.
    TaskbarButton,
    /// The name of the workspace shown, on the taskbar.
    TaskbarWorkspace,
    /// The clock on the taskbar.
    TaskbarClock,
    /// One window's entry in the taskbar's window list.
    WindowListEntry,
    /// The window list when it lists no window.
    WindowListEmpty,
    /// A menu's title.
    MenuTitle,
    /// A menu's body, behind its items, and its outline.
    MenuFrame,
    /// A menu's item; its `hovered` state is the highlighted one.
    MenuItem,
    /// The line of text a dialog edits, such as the run dialog's command.
    DialogField,
    /// The outline a resize shows while the window keeps its size: its
    /// `border` is the lines' width and color.
    Wireframe,
    /// A tooltip: a title too long for its place, or a tray item's text.
    Tooltip,
    /// vaht's warning about its own configuration. vaht draws it with the
    /// built-in style whatever the user's says, so a broken style cannot hide
    /// what is broken.
    Warning,
}

impl StyleElement {
    /// Returns the element's name in style files, a dotted path.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Titlebar => "window.titlebar",
            Self::Label => "window.label",
            Self::Button => "window.button",
            Self::Handle => "window.handle",
            Self::Grip => "window.grip",
            Self::Border => "window.border",
            Self::Taskbar => "taskbar",
            Self::TaskbarButton => "taskbar.button",
            Self::TaskbarWorkspace => "taskbar.workspace",
            Self::TaskbarClock => "taskbar.clock",
            Self::WindowListEntry => "taskbar.window-list.entry",
            Self::WindowListEmpty => "taskbar.window-list.empty",
            Self::MenuTitle => "menu.title",
            Self::MenuFrame => "menu.frame",
            Self::MenuItem => "menu.item",
            Self::DialogField => "dialog.field",
            Self::Wireframe => "wireframe",
            Self::Tooltip => "tooltip",
            Self::Warning => "warning",
        }
    }

    /// Returns the element named `path` in style files.
    #[must_use]
    pub fn at_path(path: &str) -> Option<Self> {
        <Self as strum::IntoEnumIterator>::iter().find(|element| element.path() == path)
    }
}

/// The state a widget is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter)]
pub enum WidgetState {
    /// Part of the focused window, or otherwise active.
    Focused,
    /// Part of an unfocused window.
    Unfocused,
    /// Held down by the pointer.
    Pressed,
    /// Under the pointer, or chosen with the keyboard: a menu's highlighted
    /// item.
    Hovered,
    /// Not usable now: a menu item that does nothing.
    Disabled,
}

impl WidgetState {
    /// Returns the state's name in style files.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Focused => "focused",
            Self::Unfocused => "unfocused",
            Self::Pressed => "pressed",
            Self::Hovered => "hovered",
            Self::Disabled => "disabled",
        }
    }

    /// Returns the state named `name` in style files.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        <Self as strum::IntoEnumIterator>::iter().find(|state| state.name() == name)
    }
}

/// A line drawn around an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BorderStyle {
    /// How thick, in pixels; 0 for none.
    pub width: u32,
    /// Its color.
    pub color: Color,
}

/// The pictures drawn in place of a button's own: an image for each glyph
/// that has one (Fluxbox's `window.close.pixmap` and the like).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct GlyphImages(Vec<(Glyph, Arc<Image>)>);

impl GlyphImages {
    /// Returns the image for `glyph`, if it has one.
    #[must_use]
    pub fn get(&self, glyph: Glyph) -> Option<&Arc<Image>> {
        self.0
            .iter()
            .find(|(known, _)| *known == glyph)
            .map(|(_, image)| image)
    }

    /// Returns these with `glyph` drawn as `image`.
    #[must_use]
    pub fn with(mut self, glyph: Glyph, image: Arc<Image>) -> Self {
        self.0.retain(|(known, _)| *known != glyph);
        self.0.push((glyph, image));
        self
    }

    /// Returns whether no glyph has an image.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// How one element looks in one state, every property decided.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ElementStyle {
    /// The background.
    pub texture: Texture,
    /// How text is drawn.
    pub text: TextStyle,
    /// The color of a button's picture.
    pub glyph_color: Color,
    /// The images that take the place of a button's pictures.
    pub glyph_images: GlyphImages,
    /// Space between the edges and the content.
    pub padding: Insets,
    /// The line around it.
    pub border: BorderStyle,
    /// The height of a bar, in pixels; `None` where the element is not a
    /// bar, or the bar takes the default.
    pub height: Option<u32>,
    /// How round its rounded corners are, in pixels; 0 for square.
    pub corner_radius: u32,
    /// Which corners are rounded.
    pub rounded_corners: CornerSet,
    /// A menu item's submenu bullet.
    pub bullet: Bullet,
    /// Which side of a menu item its bullet, and a toggle's check, are on.
    pub bullet_side: MarkSide,
    /// How opaque the whole of it is drawn, 0 (clear) to 255 (opaque): the
    /// taskbar's, a menu's (its `menu.frame`) or a tooltip's.
    pub alpha: u8,
}

/// A menu's submenu bullet (Fluxbox's `menu.bullet`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Bullet {
    /// None.
    Empty,
    /// A triangle pointing outward.
    #[default]
    Triangle,
    /// A square's outline.
    Square,
    /// A small diamond.
    Diamond,
}

impl Bullet {
    /// Returns the picture of this bullet on `side`; none for no bullet.
    #[must_use]
    pub const fn glyph(self, side: MarkSide) -> Option<Glyph> {
        match (self, side) {
            (Self::Empty, _) => None,
            (Self::Triangle, MarkSide::Right) => Some(Glyph::BulletRight),
            (Self::Triangle, MarkSide::Left) => Some(Glyph::BulletLeft),
            (Self::Square, _) => Some(Glyph::BulletSquare),
            (Self::Diamond, _) => Some(Glyph::BulletDiamond),
        }
    }
}

impl ElementStyle {
    /// Returns how opaque the whole of it is drawn, 0.0 to 1.0.
    #[must_use]
    pub fn opacity(&self) -> f32 {
        f32::from(self.alpha) / 255.0
    }

    /// Returns how round each corner is.
    #[must_use]
    pub fn corner_radii(&self) -> CornerRadii {
        CornerRadii::of(self.corner_radius, self.rounded_corners.iter())
    }

    /// The style of an element nothing in the style mentions.
    fn fallback() -> Self {
        Self {
            texture: Texture::solid(Color::rgb(64, 64, 64)),
            text: TextStyle {
                font: Font {
                    family: "DejaVu Sans".to_owned(),
                    size_points: 9,
                    weight: FontWeight::Normal,
                    is_italic: false,
                },
                color: Color::WHITE,
                effect: TextEffect::None,
                justify: Justify::Left,
            },
            glyph_color: Color::WHITE,
            glyph_images: GlyphImages::default(),
            padding: Insets::default(),
            border: BorderStyle::default(),
            height: None,
            corner_radius: 0,
            rounded_corners: CornerSet::ALL,
            bullet: Bullet::Triangle,
            bullet_side: MarkSide::Right,
            alpha: u8::MAX,
        }
    }
}

/// Part of a style: the properties one layer of a style sets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ElementStylePatch {
    /// The background, if this layer sets it.
    pub texture: Option<Texture>,
    /// The typeface, if this layer sets it.
    pub font: Option<Font>,
    /// The text color, if this layer sets it.
    pub text_color: Option<Color>,
    /// The text effect, if this layer sets it.
    pub text_effect: Option<TextEffect>,
    /// The text's justification, if this layer sets it.
    pub justify: Option<Justify>,
    /// The button picture color, if this layer sets it.
    pub glyph_color: Option<Color>,
    /// The images for button pictures, which this layer adds to the ones
    /// before it.
    pub glyph_images: GlyphImages,
    /// The padding, if this layer sets it.
    pub padding: Option<Insets>,
    /// The border, if this layer sets it.
    pub border: Option<BorderStyle>,
    /// The bar height, if this layer sets it.
    pub height: Option<u32>,
    /// The corner radius, if this layer sets it.
    pub corner_radius: Option<u32>,
    /// Which corners are rounded, if this layer says.
    pub rounded_corners: Option<CornerSet>,
    /// The submenu bullet, if this layer sets it.
    pub bullet: Option<Bullet>,
    /// The bullet's side, if this layer sets it.
    pub bullet_side: Option<MarkSide>,
    /// How opaque the whole element is, if this layer sets it.
    pub alpha: Option<u8>,
}

impl ElementStylePatch {
    /// Overrides in `style` every property this patch sets.
    fn apply_to(&self, style: &mut ElementStyle) {
        if let Some(texture) = &self.texture {
            style.texture = texture.clone();
        }
        if let Some(font) = &self.font {
            style.text.font = font.clone();
        }
        if let Some(color) = self.text_color {
            style.text.color = color;
        }
        if let Some(effect) = self.text_effect {
            style.text.effect = effect;
        }
        if let Some(justify) = self.justify {
            style.text.justify = justify;
        }
        if let Some(color) = self.glyph_color {
            style.glyph_color = color;
        }
        for (glyph, image) in &self.glyph_images.0 {
            style.glyph_images =
                std::mem::take(&mut style.glyph_images).with(*glyph, Arc::clone(image));
        }
        if let Some(padding) = self.padding {
            style.padding = padding;
        }
        if let Some(border) = self.border {
            style.border = border;
        }
        if let Some(height) = self.height {
            style.height = Some(height);
        }
        if let Some(radius) = self.corner_radius {
            style.corner_radius = radius;
        }
        if let Some(bullet) = self.bullet {
            style.bullet = bullet;
        }
        if let Some(side) = self.bullet_side {
            style.bullet_side = side;
        }
        if let Some(corners) = self.rounded_corners {
            style.rounded_corners = corners;
        }
        if let Some(alpha) = self.alpha {
            style.alpha = alpha;
        }
    }
}

/// Sizes the compositor lays windows and bars out by, from the style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Metrics {
    /// The height of a window's titlebar.
    pub titlebar_height: u32,
    /// The width of the line around a window.
    pub border_width: u32,
    /// The height of the strip along a window's bottom edge.
    pub handle_height: u32,
    /// The height of the taskbar.
    pub taskbar_height: u32,
    /// How round a window frame's corners are.
    pub frame_corners: CornerRadii,
    /// How menus are laid out.
    pub menu: MenuMetrics,
}

/// Sizes menus are laid out by, from the style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MenuMetrics {
    /// The height of a menu's title.
    pub title_height: u32,
    /// The height of each item.
    pub item_height: u32,
    /// The width of the outline around a menu, and under its title.
    pub border_width: u32,
    /// How round a menu's corners are.
    pub corners: CornerRadii,
    /// A submenu item's bullet.
    pub bullet: Bullet,
    /// Which side bullets and checks are on.
    pub bullet_side: MarkSide,
}

/// A style as written: four layers of patches, not yet resolved.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleLayers {
    /// Applies to everything.
    pub default: ElementStylePatch,
    /// Applies to every element in one state.
    pub by_state: HashMap<WidgetState, ElementStylePatch>,
    /// Applies to one element in every state.
    pub by_element: HashMap<StyleElement, ElementStylePatch>,
    /// Applies to one element in one state.
    pub by_element_state: HashMap<(StyleElement, WidgetState), ElementStylePatch>,
}

/// A resolved style: a complete style for every element in every state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    /// The style of each element in each state.
    styles: HashMap<(StyleElement, WidgetState), ElementStyle>,
    /// What each element's `hovered` section changes: laid over the element's
    /// look in its own state while the pointer is on it, so a style that
    /// has none looks the same hovered or not.
    hovering: HashMap<StyleElement, ElementStylePatch>,
}

impl Style {
    /// The height of a bar whose style sets none, in pixels.
    pub const DEFAULT_BAR_HEIGHT: u32 = 20;

    /// The name of the default style: `styles/vaht.toml`, which vaht has a copy
    /// of built in, so it needs no file.
    pub const DEFAULT_NAME: &str = "vaht";

    /// The default style, as a style file: `data/styles/vaht.toml`, the one
    /// file that is both built in and installed for people to read and copy.
    pub const BUILT_IN: &str = include_str!("../../../data/styles/vaht.toml");

    /// Resolves `layers`, applying them in cascade order.
    #[must_use]
    pub fn resolve(layers: &StyleLayers) -> Self {
        let mut styles = HashMap::new();
        for element in <StyleElement as strum::IntoEnumIterator>::iter() {
            for state in <WidgetState as strum::IntoEnumIterator>::iter() {
                let mut style = ElementStyle::fallback();
                layers.default.apply_to(&mut style);
                let patches = [
                    layers.by_state.get(&state),
                    layers.by_element.get(&element),
                    layers.by_element_state.get(&(element, state)),
                ];
                for patch in patches.into_iter().flatten() {
                    patch.apply_to(&mut style);
                }
                styles.insert((element, state), style);
            }
        }
        let hovering = layers
            .by_element_state
            .iter()
            .filter(|((_, state), _)| *state == WidgetState::Hovered)
            .map(|((element, _), patch)| (*element, patch.clone()))
            .collect();
        Self { styles, hovering }
    }

    /// Returns the built-in style: dark grey gradients with white, haloed
    /// text, after the `nacitar_fbstyle` Fluxbox style.
    ///
    /// # Panics
    ///
    /// Panics if the built-in style file is invalid; a unit test makes sure
    /// it is not.
    #[must_use]
    pub fn built_in() -> Self {
        Self::resolve(
            &StyleLayers::from_toml(Self::BUILT_IN).expect("the built-in style is valid (tested)"),
        )
    }

    /// Returns how `element` looks in `state`.
    ///
    /// # Panics
    ///
    /// Never: resolution gives every element a style in every state.
    #[must_use]
    pub fn element_style(&self, element: StyleElement, state: WidgetState) -> &ElementStyle {
        self.styles
            .get(&(element, state))
            .expect("resolution styles every element in every state")
    }

    /// Returns how `element` looks in `state`, with the pointer on it if
    /// `is_hovered`: its own look, with what its `hovered` section sets laid
    /// over it.
    #[must_use]
    pub fn element_style_under_pointer(
        &self,
        element: StyleElement,
        state: WidgetState,
        is_hovered: bool,
    ) -> Cow<'_, ElementStyle> {
        let style = self.element_style(element, state);
        match self.hovering.get(&element).filter(|_| is_hovered) {
            Some(patch) => {
                let mut hovered = style.clone();
                patch.apply_to(&mut hovered);
                Cow::Owned(hovered)
            }
            None => Cow::Borrowed(style),
        }
    }

    /// Returns the sizes the compositor lays windows and bars out by: the
    /// focused styles' heights and border width.
    #[must_use]
    pub fn metrics(&self) -> Metrics {
        let focused = |element| self.element_style(element, WidgetState::Focused);
        let bar = |element| focused(element).height.unwrap_or(Self::DEFAULT_BAR_HEIGHT);
        Metrics {
            titlebar_height: bar(StyleElement::Titlebar),
            border_width: focused(StyleElement::Border).border.width,
            handle_height: focused(StyleElement::Handle).height.unwrap_or(0),
            taskbar_height: bar(StyleElement::Taskbar),
            frame_corners: focused(StyleElement::Border).corner_radii(),
            menu: MenuMetrics {
                title_height: bar(StyleElement::MenuTitle),
                item_height: bar(StyleElement::MenuItem),
                border_width: focused(StyleElement::MenuFrame).border.width,
                corners: focused(StyleElement::MenuFrame).corner_radii(),
                bullet: focused(StyleElement::MenuItem).bullet,
                bullet_side: focused(StyleElement::MenuItem).bullet_side,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layers whose only setting is the default text color, to build on.
    fn layers_with_default_red_text() -> StyleLayers {
        StyleLayers {
            default: ElementStylePatch {
                text_color: Some(Color::rgb(255, 0, 0)),
                ..ElementStylePatch::default()
            },
            by_state: HashMap::new(),
            by_element: HashMap::new(),
            by_element_state: HashMap::new(),
        }
    }

    /// A patch setting only the text color.
    fn text_color(color: Color) -> ElementStylePatch {
        ElementStylePatch {
            text_color: Some(color),
            ..ElementStylePatch::default()
        }
    }

    #[test]
    fn each_layer_overrides_the_ones_before_it() {
        let (green, blue, white) = (Color::rgb(0, 255, 0), Color::rgb(0, 0, 255), Color::WHITE);
        let mut layers = layers_with_default_red_text();
        layers
            .by_state
            .insert(WidgetState::Unfocused, text_color(green));
        layers
            .by_element
            .insert(StyleElement::Label, text_color(blue));
        layers.by_element_state.insert(
            (StyleElement::Label, WidgetState::Pressed),
            text_color(white),
        );
        let style = Style::resolve(&layers);
        let color = |element, state| style.element_style(element, state).text.color;

        assert_eq!(
            color(StyleElement::Button, WidgetState::Focused),
            Color::rgb(255, 0, 0)
        );
        assert_eq!(
            color(StyleElement::Button, WidgetState::Unfocused),
            green,
            "state beats default"
        );
        assert_eq!(
            color(StyleElement::Label, WidgetState::Unfocused),
            blue,
            "element beats state"
        );
        assert_eq!(
            color(StyleElement::Label, WidgetState::Pressed),
            white,
            "element and state beat all"
        );
    }

    #[test]
    fn a_patch_changes_only_what_it_sets() {
        let mut layers = layers_with_default_red_text();
        layers.by_element.insert(
            StyleElement::Button,
            ElementStylePatch {
                glyph_color: Some(Color::BLACK),
                ..ElementStylePatch::default()
            },
        );
        let style = Style::resolve(&layers)
            .element_style(StyleElement::Button, WidgetState::Focused)
            .clone();
        assert_eq!(style.glyph_color, Color::BLACK);
        assert_eq!(
            style.text.color,
            Color::rgb(255, 0, 0),
            "the default's text color survives"
        );
    }

    #[test]
    fn the_built_in_style_styles_focus_and_pressing_differently() {
        let style = Style::built_in();
        let texture = |state| {
            style
                .element_style(StyleElement::Button, state)
                .texture
                .clone()
        };
        assert_ne!(
            texture(WidgetState::Focused),
            texture(WidgetState::Unfocused)
        );
        assert_eq!(
            texture(WidgetState::Pressed).bevel,
            crate::texture::Bevel::Sunken
        );
    }

    #[test]
    fn the_metrics_come_from_the_bars_and_the_border() {
        let metrics = Style::built_in().metrics();
        assert_eq!(
            metrics,
            Metrics {
                titlebar_height: 20,
                border_width: 1,
                handle_height: 1,
                taskbar_height: 20,
                frame_corners: CornerRadii::SQUARE,
                menu: MenuMetrics {
                    title_height: 20,
                    item_height: 20,
                    border_width: 1,
                    corners: CornerRadii::of(8, <crate::Corner as strum::IntoEnumIterator>::iter()),
                    bullet: Bullet::Triangle,
                    bullet_side: MarkSide::Right,
                },
            }
        );
    }

    #[test]
    fn every_element_and_state_has_a_name_that_finds_it() {
        for element in <StyleElement as strum::IntoEnumIterator>::iter() {
            assert_eq!(StyleElement::at_path(element.path()), Some(element));
        }
        for state in <WidgetState as strum::IntoEnumIterator>::iter() {
            assert_eq!(WidgetState::named(state.name()), Some(state));
        }
    }
}

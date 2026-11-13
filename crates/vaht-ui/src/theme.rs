//! What every themable thing looks like in every state.
//!
//! A [`Theme`] answers one question: the [`Style`] of a [`ThemeElement`] in
//! a [`WidgetState`]. It is resolved once from [`ThemeLayers`], the four
//! layers a theme file writes (02 §10.1), each overriding the one before:
//!
//! 1. the default, for everything;
//! 2. one state, for every element;
//! 3. one element, in every state;
//! 4. one element in one state.
//!
//! These replace Fluxbox's wildcard resource matching. Drawing never cascades:
//! it looks the resolved style up. Theme files are TOML (`theme/file.rs`
//! reads them); the built-in theme is one too, `theme/built_in.toml`, after
//! the `nacitar_fbstyle` Fluxbox style.

mod file;

use std::collections::HashMap;

use crate::color::Color;
use crate::corner::{CornerRadii, CornerSet};
use crate::geometry::Insets;
use crate::text::{Font, FontWeight, Justify, TextEffect, TextStyle};
use crate::texture::Texture;
use crate::widget::{Glyph, MarkSide};

/// Something a theme styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::EnumIter)]
pub enum ThemeElement {
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
}

impl ThemeElement {
    /// Returns the element's name in theme files, a dotted path.
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
        }
    }

    /// Returns the element named `path` in theme files.
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
    /// Returns the state's name in theme files.
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

    /// Returns the state named `name` in theme files.
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

/// How one element looks in one state, every property decided.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Style {
    /// The background.
    pub texture: Texture,
    /// How text is drawn.
    pub text: TextStyle,
    /// The color of a button's picture.
    pub glyph_color: Color,
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

impl Style {
    /// Returns how round each corner is.
    #[must_use]
    pub fn corner_radii(&self) -> CornerRadii {
        CornerRadii::of(self.corner_radius, self.rounded_corners.iter())
    }

    /// The style of an element nothing in the theme mentions.
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
            padding: Insets::default(),
            border: BorderStyle::default(),
            height: None,
            corner_radius: 0,
            rounded_corners: CornerSet::ALL,
            bullet: Bullet::Triangle,
            bullet_side: MarkSide::Right,
        }
    }
}

/// Part of a style: the properties one layer of a theme sets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StylePatch {
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
}

impl StylePatch {
    /// Overrides in `style` every property this patch sets.
    fn apply_to(&self, style: &mut Style) {
        if let Some(texture) = self.texture {
            style.texture = texture;
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
    }
}

/// Sizes the compositor lays windows and bars out by, from the theme.
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

/// Sizes menus are laid out by, from the theme.
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

/// A theme as written: four layers of patches, not yet resolved.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThemeLayers {
    /// Applies to everything.
    pub default: StylePatch,
    /// Applies to every element in one state.
    pub by_state: HashMap<WidgetState, StylePatch>,
    /// Applies to one element in every state.
    pub by_element: HashMap<ThemeElement, StylePatch>,
    /// Applies to one element in one state.
    pub by_element_state: HashMap<(ThemeElement, WidgetState), StylePatch>,
}

/// A resolved theme: a complete style for every element in every state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// The style of each element in each state.
    styles: HashMap<(ThemeElement, WidgetState), Style>,
}

impl Theme {
    /// The height of a bar whose theme sets none, in pixels.
    pub const DEFAULT_BAR_HEIGHT: u32 = 20;

    /// The built-in theme, as a theme file.
    pub const BUILT_IN: &str = include_str!("theme/built_in.toml");

    /// Resolves `layers`, applying them in cascade order.
    #[must_use]
    pub fn resolve(layers: &ThemeLayers) -> Self {
        let mut styles = HashMap::new();
        for element in <ThemeElement as strum::IntoEnumIterator>::iter() {
            for state in <WidgetState as strum::IntoEnumIterator>::iter() {
                let mut style = Style::fallback();
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
        Self { styles }
    }

    /// Returns the built-in theme: dark grey gradients with white, haloed
    /// text, after the `nacitar_fbstyle` Fluxbox style.
    ///
    /// # Panics
    ///
    /// Panics if the built-in theme file is invalid; a unit test makes sure
    /// it is not.
    #[must_use]
    pub fn built_in() -> Self {
        Self::resolve(
            &ThemeLayers::from_toml(Self::BUILT_IN).expect("the built-in theme is valid (tested)"),
        )
    }

    /// Returns how `element` looks in `state`.
    ///
    /// # Panics
    ///
    /// Never: resolution gives every element a style in every state.
    #[must_use]
    pub fn style(&self, element: ThemeElement, state: WidgetState) -> &Style {
        self.styles
            .get(&(element, state))
            .expect("resolution styles every element in every state")
    }

    /// Returns the sizes the compositor lays windows and bars out by: the
    /// focused styles' heights and border width.
    #[must_use]
    pub fn metrics(&self) -> Metrics {
        let focused = |element| self.style(element, WidgetState::Focused);
        let bar = |element| focused(element).height.unwrap_or(Self::DEFAULT_BAR_HEIGHT);
        Metrics {
            titlebar_height: bar(ThemeElement::Titlebar),
            border_width: focused(ThemeElement::Border).border.width,
            handle_height: focused(ThemeElement::Handle).height.unwrap_or(0),
            taskbar_height: bar(ThemeElement::Taskbar),
            frame_corners: focused(ThemeElement::Border).corner_radii(),
            menu: MenuMetrics {
                title_height: bar(ThemeElement::MenuTitle),
                item_height: bar(ThemeElement::MenuItem),
                border_width: focused(ThemeElement::MenuFrame).border.width,
                corners: focused(ThemeElement::MenuFrame).corner_radii(),
                bullet: focused(ThemeElement::MenuItem).bullet,
                bullet_side: focused(ThemeElement::MenuItem).bullet_side,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layers whose only setting is the default text color, to build on.
    fn layers_with_default_red_text() -> ThemeLayers {
        ThemeLayers {
            default: StylePatch {
                text_color: Some(Color::rgb(255, 0, 0)),
                ..StylePatch::default()
            },
            by_state: HashMap::new(),
            by_element: HashMap::new(),
            by_element_state: HashMap::new(),
        }
    }

    /// A patch setting only the text color.
    fn text_color(color: Color) -> StylePatch {
        StylePatch {
            text_color: Some(color),
            ..StylePatch::default()
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
            .insert(ThemeElement::Label, text_color(blue));
        layers.by_element_state.insert(
            (ThemeElement::Label, WidgetState::Pressed),
            text_color(white),
        );
        let theme = Theme::resolve(&layers);
        let color = |element, state| theme.style(element, state).text.color;

        assert_eq!(
            color(ThemeElement::Button, WidgetState::Focused),
            Color::rgb(255, 0, 0)
        );
        assert_eq!(
            color(ThemeElement::Button, WidgetState::Unfocused),
            green,
            "state beats default"
        );
        assert_eq!(
            color(ThemeElement::Label, WidgetState::Unfocused),
            blue,
            "element beats state"
        );
        assert_eq!(
            color(ThemeElement::Label, WidgetState::Pressed),
            white,
            "element and state beat all"
        );
    }

    #[test]
    fn a_patch_changes_only_what_it_sets() {
        let mut layers = layers_with_default_red_text();
        layers.by_element.insert(
            ThemeElement::Button,
            StylePatch {
                glyph_color: Some(Color::BLACK),
                ..StylePatch::default()
            },
        );
        let style = Theme::resolve(&layers)
            .style(ThemeElement::Button, WidgetState::Focused)
            .clone();
        assert_eq!(style.glyph_color, Color::BLACK);
        assert_eq!(
            style.text.color,
            Color::rgb(255, 0, 0),
            "the default's text color survives"
        );
    }

    #[test]
    fn the_built_in_theme_styles_focus_and_pressing_differently() {
        let theme = Theme::built_in();
        let texture = |state| theme.style(ThemeElement::Button, state).texture;
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
        let metrics = Theme::built_in().metrics();
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
        for element in <ThemeElement as strum::IntoEnumIterator>::iter() {
            assert_eq!(ThemeElement::at_path(element.path()), Some(element));
        }
        for state in <WidgetState as strum::IntoEnumIterator>::iter() {
            assert_eq!(WidgetState::named(state.name()), Some(state));
        }
    }
}

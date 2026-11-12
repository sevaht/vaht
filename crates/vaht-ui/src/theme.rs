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
//! it looks the resolved style up.

use std::collections::HashMap;

use crate::color::Color;
use crate::geometry::Insets;
use crate::text::{Font, FontWeight, Justify, TextEffect, TextStyle};
use crate::texture::{Bevel, GradientDirection, Texture};

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
}

impl Style {
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
    }
}

/// Sizes a theme sets that belong to no single element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Metrics {
    /// The height of a window's titlebar.
    pub titlebar_height: u32,
    /// The width of the line around a window.
    pub border_width: u32,
    /// The height of the strip along a window's bottom edge.
    pub handle_height: u32,
}

/// A theme as written: four layers of patches, not yet resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeLayers {
    /// Applies to everything.
    pub default: StylePatch,
    /// Applies to every element in one state.
    pub by_state: HashMap<WidgetState, StylePatch>,
    /// Applies to one element in every state.
    pub by_element: HashMap<ThemeElement, StylePatch>,
    /// Applies to one element in one state.
    pub by_element_state: HashMap<(ThemeElement, WidgetState), StylePatch>,
    /// The theme's sizes.
    pub metrics: Metrics,
}

/// A resolved theme: a complete style for every element in every state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// The style of each element in each state.
    styles: HashMap<(ThemeElement, WidgetState), Style>,
    /// The theme's sizes.
    metrics: Metrics,
}

impl Theme {
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
        Self {
            styles,
            metrics: layers.metrics,
        }
    }

    /// Returns the built-in theme: dark grey gradients with white, haloed
    /// text, after the `nacitar_fbstyle` Fluxbox style.
    #[must_use]
    pub fn built_in() -> Self {
        Self::resolve(&built_in_layers())
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

    /// Returns the theme's sizes.
    #[must_use]
    pub const fn metrics(&self) -> Metrics {
        self.metrics
    }
}

/// The built-in theme's greys, named by the X11 names its original style
/// used (`grey55` is 55% white).
mod grey {
    use crate::color::Color;

    /// `grey5`.
    pub(super) const GREY_5: Color = Color::rgb(13, 13, 13);
    /// `grey20`.
    pub(super) const GREY_20: Color = Color::rgb(51, 51, 51);
    /// `grey30`.
    pub(super) const GREY_30: Color = Color::rgb(77, 77, 77);
    /// `grey40`.
    pub(super) const GREY_40: Color = Color::rgb(102, 102, 102);
    /// `grey55`.
    pub(super) const GREY_55: Color = Color::rgb(140, 140, 140);
    /// `grey60`.
    pub(super) const GREY_60: Color = Color::rgb(153, 153, 153);
}

/// The layers of the built-in theme.
fn built_in_layers() -> ThemeLayers {
    ThemeLayers {
        default: StylePatch {
            texture: Some(Texture::gradient(
                GradientDirection::Vertical,
                grey::GREY_30,
                grey::GREY_5,
            )),
            font: Some(Font {
                family: "DejaVu Sans".to_owned(),
                size_points: 8,
                weight: FontWeight::Bold,
                is_italic: false,
            }),
            text_color: Some(Color::WHITE),
            text_effect: Some(TextEffect::Halo(Color::BLACK)),
            justify: Some(Justify::Center),
            glyph_color: Some(Color::WHITE),
            padding: Some(Insets::uniform(0)),
        },
        by_state: built_in_state_patches(),
        by_element: built_in_element_patches(),
        by_element_state: HashMap::new(),
        metrics: Metrics {
            titlebar_height: 20,
            border_width: 1,
            handle_height: 1,
        },
    }
}

/// The built-in theme's patches by state: a lighter focused gradient, grey
/// unfocused text, and a sunken cross-diagonal look while pressed.
fn built_in_state_patches() -> HashMap<WidgetState, StylePatch> {
    HashMap::from([
        (
            WidgetState::Focused,
            StylePatch {
                texture: Some(Texture::gradient(
                    GradientDirection::Vertical,
                    grey::GREY_55,
                    grey::GREY_5,
                )),
                ..StylePatch::default()
            },
        ),
        (
            WidgetState::Unfocused,
            StylePatch {
                text_color: Some(grey::GREY_55),
                glyph_color: Some(grey::GREY_55),
                ..StylePatch::default()
            },
        ),
        (
            WidgetState::Pressed,
            StylePatch {
                texture: Some(
                    Texture::gradient(
                        GradientDirection::CrossDiagonal,
                        grey::GREY_20,
                        grey::GREY_40,
                    )
                    .with_bevel(Bevel::Sunken),
                ),
                ..StylePatch::default()
            },
        ),
    ])
}

/// The built-in theme's patches by element: raised white handles and grips,
/// a grey border, and padding around button pictures and titles.
fn built_in_element_patches() -> HashMap<ThemeElement, StylePatch> {
    let handle = StylePatch {
        texture: Some(
            Texture::gradient(GradientDirection::Vertical, Color::WHITE, grey::GREY_60)
                .with_bevel(Bevel::Raised),
        ),
        ..StylePatch::default()
    };
    HashMap::from([
        (ThemeElement::Handle, handle.clone()),
        (ThemeElement::Grip, handle),
        (
            ThemeElement::Border,
            StylePatch {
                texture: Some(Texture::solid(grey::GREY_60)),
                ..StylePatch::default()
            },
        ),
        (
            ThemeElement::Button,
            StylePatch {
                padding: Some(Insets::uniform(5)),
                ..StylePatch::default()
            },
        ),
        (
            ThemeElement::Label,
            StylePatch {
                padding: Some(Insets {
                    left: 4,
                    right: 4,
                    ..Insets::default()
                }),
                ..StylePatch::default()
            },
        ),
    ])
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
            metrics: Metrics {
                titlebar_height: 20,
                border_width: 1,
                handle_height: 1,
            },
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
        assert_eq!(texture(WidgetState::Pressed).bevel, Bevel::Sunken);
    }
}

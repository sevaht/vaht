//! A line around an area: a resize's outline, a tooltip's edge.
//!
//! Four straight lines in the border a style element gives (the
//! `wireframe`: white at 80%, two pixels wide, as fluxland draws it, in the
//! built-in style), stopping short of rounded corners, where an arc each
//! joins them ([`CornerTexture`]). [`Outline::lines_around`] is where such
//! lines go, which menus' outlines share.

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::utils::{Logical, Rectangle, Scale};
use vaht_ui::{Corner, CornerArc, CornerRadii, Style, StyleElement, WidgetState};

use super::{CornerTexture, SceneElement, SceneRenderer};
use crate::window_manager::Area;

/// The outline's lines and arcs, as the renderer takes them.
#[derive(Debug)]
pub(crate) struct Outline {
    /// What is outlined, while an outline shows.
    shown: Option<Outlined>,
    /// Each line's buffer.
    buffers: [SolidColorBuffer; 4],
    /// Each corner's arc, in [`Corner`]'s order.
    arcs: [CornerTexture; 4],
}

/// An area outlined.
#[derive(Debug, Clone, Copy)]
struct Outlined {
    /// The area.
    area: Area,
    /// How round its corners are.
    corners: CornerRadii,
    /// Its lines: top, bottom, left, right.
    lines: [Rectangle<i32, Logical>; 4],
}

impl Default for Outline {
    fn default() -> Self {
        Self {
            shown: None,
            buffers: std::array::from_fn(|_| SolidColorBuffer::default()),
            arcs: std::array::from_fn(|_| CornerTexture::new()),
        }
    }
}

impl Outline {
    /// Shows the outline of `area`, or none, in `style`'s wireframe.
    pub(crate) fn update(&mut self, area: Option<Area>, style: &Style) {
        self.update_as(area, style, StyleElement::Wireframe, CornerRadii::SQUARE);
    }

    /// Shows the outline of `area`, or none, in the border `style` gives
    /// `element`, its corners as round as `corners` says.
    pub(crate) fn update_as(
        &mut self,
        area: Option<Area>,
        style: &Style,
        element: StyleElement,
        corners: CornerRadii,
    ) {
        let border = style.element_style(element, WidgetState::Focused).border;
        let Some(area) = area else {
            self.shown = None;
            return;
        };
        let width = i32::try_from(border.width)
            .unwrap_or(i32::MAX)
            .min(area.size.w)
            .min(area.size.h);
        let lines = Self::lines_around(area, width, corners);
        // Premultiplied, as solid buffers take colors.
        let color = border.color;
        let alpha = f32::from(color.alpha) / 255.0;
        let [red, green, blue] =
            [color.red, color.green, color.blue].map(|channel| f32::from(channel) / 255.0 * alpha);
        for (buffer, line) in self.buffers.iter_mut().zip(&lines) {
            buffer.update(line.size, [red, green, blue, alpha]);
        }
        for (texture, corner) in self
            .arcs
            .iter_mut()
            .zip(<Corner as strum::IntoEnumIterator>::iter())
        {
            texture.update(CornerArc {
                corner,
                radius: corners.radius(corner),
                line_width: u32::try_from(width).unwrap_or(0),
                color,
            });
        }
        self.shown = Some(Outlined {
            area,
            corners,
            lines,
        });
    }

    /// Returns where the lines around `outer` go, `width` wide, stopping
    /// short of its rounded `corners`: top, bottom, left, right.
    pub(crate) fn lines_around(
        outer: Area,
        width: i32,
        corners: CornerRadii,
    ) -> [Rectangle<i32, Logical>; 4] {
        let radius = |corner| i32::try_from(corners.radius(corner)).unwrap_or(0);
        let (top_left, top_right) = (radius(Corner::TopLeft), radius(Corner::TopRight));
        let (bottom_left, bottom_right) = (radius(Corner::BottomLeft), radius(Corner::BottomRight));
        let (left, top) = (outer.loc.x, outer.loc.y);
        let (right, bottom) = (left + outer.size.w, top + outer.size.h);
        let line =
            |left, top, width, height| Rectangle::new((left, top).into(), (width, height).into());
        [
            line(
                left + top_left,
                top,
                outer.size.w - top_left - top_right,
                width,
            ),
            line(
                left + bottom_left,
                bottom - width,
                outer.size.w - bottom_left - bottom_right,
                width,
            ),
            line(
                left,
                top + top_left,
                width,
                outer.size.h - top_left - bottom_left,
            ),
            line(
                right - width,
                top + top_right,
                width,
                outer.size.h - top_right - bottom_right,
            ),
        ]
    }

    /// Returns where the square of `corner`'s arc is: as wide as its
    /// radius, in that corner of `outer`.
    pub(crate) fn corner_square(outer: Area, corner: Corner, corners: CornerRadii) -> Area {
        let radius = i32::try_from(corners.radius(corner)).unwrap_or(0);
        let x = if corner.is_left() {
            outer.loc.x
        } else {
            outer.loc.x + outer.size.w - radius
        };
        let y = if corner.is_top() {
            outer.loc.y
        } else {
            outer.loc.y + outer.size.h - radius
        };
        Rectangle::new((x, y).into(), (radius, radius).into())
    }

    /// Returns the elements that draw the outline, if one shows, `opacity`
    /// opaque (1.0 for wholly).
    pub(crate) fn render_elements<R>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
        opacity: f32,
    ) -> Vec<SceneElement<R>>
    where
        R: SceneRenderer,
        R::TextureId: Send + Clone + 'static,
    {
        let Some(Outlined {
            area,
            corners,
            lines,
        }) = &self.shown
        else {
            return Vec::new();
        };
        let mut elements: Vec<SceneElement<R>> = self
            .arcs
            .iter()
            .zip(<Corner as strum::IntoEnumIterator>::iter())
            .filter_map(|(texture, corner)| {
                texture.render_element(
                    renderer,
                    Self::corner_square(*area, corner, *corners),
                    scale,
                    opacity,
                )
            })
            .collect();
        elements.extend(
            self.buffers
                .iter()
                .zip(lines)
                .filter(|(_, line)| !line.is_empty())
                .map(|(buffer, line)| {
                    SceneElement::Solid(SolidColorRenderElement::from_buffer(
                        buffer,
                        line.loc.to_physical_precise_round(scale),
                        scale,
                        opacity,
                        Kind::Unspecified,
                    ))
                }),
        );
        elements
    }
}

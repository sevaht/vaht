//! The outline an outline resize shows: where the window's frame will go.
//!
//! Four lines in the theme's `wireframe` border (white at 80%, two pixels
//! wide, as fluxland draws it, in the built-in theme); drawn over
//! everything but the menus.

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::utils::{Logical, Rectangle, Scale};

use vaht_ui::{Theme, ThemeElement, WidgetState};

use super::SceneElement;
use crate::window_manager::Area;

/// The outline's lines, as the renderer takes them.
#[derive(Debug, Default)]
pub(crate) struct Outline {
    /// Where each line goes: top, bottom, left, right; none while no
    /// outline shows.
    lines: Option<[Rectangle<i32, Logical>; 4]>,
    /// Each line's buffer.
    buffers: [SolidColorBuffer; 4],
}

impl Outline {
    /// Shows the outline of `area`, or none, in `theme`'s wireframe.
    pub(crate) fn update(&mut self, area: Option<Area>, theme: &Theme) {
        self.update_as(area, theme, ThemeElement::Wireframe);
    }

    /// Shows the outline of `area`, or none, in the border `theme` gives
    /// `element`: a resize's wireframe, a tooltip's edge.
    pub(crate) fn update_as(&mut self, area: Option<Area>, theme: &Theme, element: ThemeElement) {
        let border = theme.style(element, WidgetState::Focused).border;
        let width = i32::try_from(border.width).unwrap_or(i32::MAX);
        // Premultiplied, as solid buffers take colors.
        let color = border.color;
        let alpha = f32::from(color.alpha) / 255.0;
        let [red, green, blue] =
            [color.red, color.green, color.blue].map(|channel| f32::from(channel) / 255.0 * alpha);
        let render_color = [red, green, blue, alpha];
        self.lines = area.map(|area| {
            let thickness = width.min(area.size.w).min(area.size.h);
            let (left, top) = (area.loc.x, area.loc.y);
            let (width, height) = (area.size.w, area.size.h);
            let line = |left, top, width, height| {
                Rectangle::new((left, top).into(), (width, height).into())
            };
            let side_height = height - 2 * thickness;
            [
                line(left, top, width, thickness),
                line(left, top + height - thickness, width, thickness),
                line(left, top + thickness, thickness, side_height),
                line(
                    left + width - thickness,
                    top + thickness,
                    thickness,
                    side_height,
                ),
            ]
        });
        if let Some(lines) = &self.lines {
            for (buffer, line) in self.buffers.iter_mut().zip(lines) {
                buffer.update(line.size, render_color);
            }
        }
    }

    /// Returns the elements that draw the outline, if one shows.
    pub(crate) fn render_elements<R>(&self, scale: Scale<f64>) -> Vec<SceneElement<R>>
    where
        R: smithay::backend::renderer::Renderer
            + smithay::backend::renderer::ImportAll
            + smithay::backend::renderer::ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some(lines) = &self.lines else {
            return Vec::new();
        };
        self.buffers
            .iter()
            .zip(lines)
            .filter(|(_, line)| !line.is_empty())
            .map(|(buffer, line)| {
                SceneElement::Solid(SolidColorRenderElement::from_buffer(
                    buffer,
                    line.loc.to_physical_precise_round(scale),
                    scale,
                    1.0,
                    Kind::Unspecified,
                ))
            })
            .collect()
    }
}

//! The outline an outline resize shows: where the window's frame will go.
//!
//! Four lines, white at 80%, two pixels wide, as fluxland draws its
//! wireframe; drawn over everything but the menus.

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::utils::{Logical, Rectangle, Scale};

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
    /// How wide the lines are, in pixels.
    const WIDTH: i32 = 2;

    /// The lines' color: white, mostly opaque.
    const COLOR: [f32; 4] = [0.8, 0.8, 0.8, 0.8];

    /// Shows the outline of `area`, or none.
    pub(crate) fn update(&mut self, area: Option<Area>) {
        self.lines = area.map(|area| {
            let thickness = Self::WIDTH.min(area.size.w).min(area.size.h);
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
                buffer.update(line.size, Self::COLOR);
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

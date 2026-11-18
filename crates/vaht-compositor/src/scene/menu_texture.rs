//! One open menu, as the renderer takes it.
//!
//! A menu is drawn as a window frame is: its panel's pixels
//! ([`PanelTexture`]), a colored buffer per line (the outline in
//! `menu.frame`'s border color, separators in its text color, as Fluxbox
//! draws them) and an arc at each rounded corner ([`CornerTexture`]).

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::utils::{Logical, Rectangle, Scale};
use vaht_ui::{Color, Corner, CornerArc, Panel, Style, StyleElement, WidgetState};

use super::{CornerTexture, PanelTexture, SceneElement};
use crate::menu::{MenuContent, MenuGeometry};

/// One open menu's buffers.
#[derive(Debug)]
pub(crate) struct MenuTexture {
    /// The panel's pixels.
    panel: PanelTexture,
    /// The outline's lines and the line under the title, in
    /// [`MenuGeometry::lines`]' order.
    lines: [SolidColorBuffer; 5],
    /// The separators' lines.
    separators: Vec<SolidColorBuffer>,
    /// Each corner's arc, in [`Corner`]'s order.
    corners: [CornerTexture; 4],
    /// How opaque the menu is drawn, as the style says.
    opacity: f32,
}

impl MenuTexture {
    /// Creates empty buffers.
    pub(crate) fn new() -> Self {
        Self {
            panel: PanelTexture::new(),
            lines: std::array::from_fn(|_| SolidColorBuffer::default()),
            separators: Vec::new(),
            corners: std::array::from_fn(|_| CornerTexture::new()),
            opacity: 1.0,
        }
    }

    /// Copies `panel`'s pixels if they are newer, and sizes and colors the
    /// lines for `geometry` as `style` styles menus.
    pub(crate) fn update(
        &mut self,
        panel: &Panel<MenuContent>,
        geometry: &MenuGeometry,
        style: &Style,
    ) {
        self.panel.upload(panel);
        let frame = style.element_style(StyleElement::MenuFrame, WidgetState::Focused);
        self.opacity = frame.opacity();
        let outline = frame.border.color;
        for (buffer, line) in self.lines.iter_mut().zip(&geometry.lines) {
            buffer.update(line.size, Self::to_render_color(outline));
        }
        self.separators
            .resize_with(geometry.separators.len(), SolidColorBuffer::default);
        for (buffer, line) in self.separators.iter_mut().zip(&geometry.separators) {
            buffer.update(line.size, Self::to_render_color(frame.text.color));
        }
        for (texture, corner) in self
            .corners
            .iter_mut()
            .zip(<Corner as strum::IntoEnumIterator>::iter())
        {
            texture.update(CornerArc {
                corner,
                radius: geometry.corners.radius(corner),
                line_width: geometry.line_width,
                color: outline,
            });
        }
    }

    /// Returns the elements that draw the menu at `geometry`, topmost
    /// first.
    pub(crate) fn render_elements<R>(
        &self,
        geometry: &MenuGeometry,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let mut elements: Vec<SceneElement<R>> = self
            .corners
            .iter()
            .zip(<Corner as strum::IntoEnumIterator>::iter())
            .filter_map(|(texture, corner)| {
                texture.render_element(
                    renderer,
                    geometry.corner_square(corner),
                    scale,
                    self.opacity,
                )
            })
            .collect();
        let solid = |buffer: &SolidColorBuffer, line: &Rectangle<i32, Logical>| {
            (!line.is_empty()).then(|| {
                SceneElement::Solid(SolidColorRenderElement::from_buffer(
                    buffer,
                    line.loc.to_physical_precise_round(scale),
                    scale,
                    self.opacity,
                    Kind::Unspecified,
                ))
            })
        };
        elements.extend(
            self.lines
                .iter()
                .zip(&geometry.lines)
                .chain(self.separators.iter().zip(&geometry.separators))
                .filter_map(|(buffer, line)| solid(buffer, line)),
        );
        elements.extend(self.panel.render_element(
            renderer,
            geometry.panel.loc,
            scale,
            self.opacity,
        ));
        elements
    }

    /// Returns `color` as solid buffers take it.
    fn to_render_color(color: Color) -> [f32; 4] {
        [color.red, color.green, color.blue, color.alpha].map(|channel| f32::from(channel) / 255.0)
    }
}

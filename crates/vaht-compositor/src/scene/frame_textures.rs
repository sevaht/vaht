//! Every window's frame, as the renderer takes it: the titlebar's and the
//! handle's pixels ([`PanelTexture`]), one colored buffer per line, and an
//! arc at each rounded corner.
//!
//! The window manager paints each titlebar and handle (`Frame`); [`FrameTextures`]
//! copies new pixels into buffers the renderer can draw and turns frames
//! into render elements. A window's buffers go when the window does: each
//! update forgets the windows that are gone.

use std::collections::HashMap;

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::utils::Scale;
use vaht_ui::{Color, Corner, CornerArc, Style, StyleElement};

use super::{CornerTexture, PanelTexture, SceneElement};
use crate::resources::{ResourceKind, ResourceLedger, Tracked};
use crate::window_manager::{FrameGeometry, Window, WindowId, WindowManager};

/// The frame buffers of every window, by window.
#[derive(Debug, Default)]
pub(crate) struct FrameTextures {
    /// Each window's buffers.
    by_window: HashMap<WindowId, FrameTexture>,
}

impl FrameTextures {
    /// Brings every window's buffers up to date with its frame, colored by
    /// `style`, and forgets the buffers of windows that are gone.
    pub(crate) fn update(
        &mut self,
        window_manager: &WindowManager,
        style: &Style,
        ledger: &ResourceLedger,
    ) {
        self.by_window
            .retain(|window, _| window_manager.window(*window).is_some());
        let metrics = window_manager.metrics();
        for window in window_manager.windows_top_down() {
            self.by_window
                .entry(window.id())
                .or_insert_with(|| FrameTexture::new(ledger))
                .update(
                    window,
                    &window.frame_geometry(metrics),
                    style,
                    window_manager.opacity(window),
                );
        }
    }

    /// Returns the elements that draw the frame of `window` at `geometry`,
    /// topmost first; none before its buffers were first updated.
    pub(crate) fn render_elements<R>(
        &self,
        window: WindowId,
        geometry: &FrameGeometry,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        self.by_window
            .get(&window)
            .map(|texture| texture.render_elements(geometry, renderer, scale))
            .unwrap_or_default()
    }
}

/// One window's frame buffers.
#[derive(Debug)]
struct FrameTexture {
    /// The titlebar's pixels.
    titlebar: PanelTexture,
    /// The handle's pixels.
    handle: PanelTexture,
    /// One colored buffer per line, in [`FrameGeometry::borders`]' order.
    borders: [SolidColorBuffer; FrameGeometry::LINES],
    /// Each corner's arc, in [`Corner`]'s order.
    corners: [CornerTexture; 4],
    /// How opaque the window is drawn, 0.0 to 1.0.
    opacity: f32,
    /// Counts the buffers while they exist.
    _tracked: Tracked,
}

impl FrameTexture {
    /// Creates empty buffers.
    fn new(ledger: &ResourceLedger) -> Self {
        Self {
            titlebar: PanelTexture::new(),
            handle: PanelTexture::new(),
            corners: std::array::from_fn(|_| CornerTexture::new()),
            borders: std::array::from_fn(|_| SolidColorBuffer::default()),
            opacity: 1.0,
            _tracked: ledger.track(ResourceKind::FrameTexture),
        }
    }

    /// Copies `window`'s titlebar and handle pixels if they are newer than
    /// the last copy, sizes and colors the lines for `geometry`, and takes
    /// the window's `opacity`.
    fn update(&mut self, window: &Window, geometry: &FrameGeometry, style: &Style, opacity: f32) {
        self.opacity = opacity;
        let frame = window.frame();
        self.titlebar.upload(frame.titlebar());
        self.handle.upload(frame.handle());
        let color = style
            .element_style(StyleElement::Border, frame.state())
            .border
            .color;
        for (buffer, rectangle) in self.borders.iter_mut().zip(geometry.borders) {
            buffer.update(rectangle.size, Self::to_render_color(color));
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
                color,
            });
        }
    }

    /// Returns the elements that draw the frame at `geometry`, topmost
    /// first.
    fn render_elements<R>(
        &self,
        geometry: &FrameGeometry,
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
        for (buffer, rectangle) in self.borders.iter().zip(geometry.borders) {
            if !rectangle.is_empty() {
                elements.push(SceneElement::Solid(SolidColorRenderElement::from_buffer(
                    buffer,
                    rectangle.loc.to_physical_precise_round(scale),
                    scale,
                    self.opacity,
                    Kind::Unspecified,
                )));
            }
        }
        // Below the lines: the grips' lines are drawn over the handle.
        elements.extend(self.titlebar.render_element(
            renderer,
            geometry.titlebar.loc,
            scale,
            self.opacity,
        ));
        if !geometry.handle.is_empty() {
            elements.extend(self.handle.render_element(
                renderer,
                geometry.handle.loc,
                scale,
                self.opacity,
            ));
        }
        elements
    }

    /// Returns `color` as solid buffers take it.
    fn to_render_color(color: Color) -> [f32; 4] {
        [color.red, color.green, color.blue, color.alpha].map(|channel| f32::from(channel) / 255.0)
    }
}

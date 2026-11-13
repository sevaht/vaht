//! Every window's frame, as the renderer takes it: the titlebar's and the
//! handle's pixels ([`PanelTexture`]), one colored buffer per line, and an
//! arc at each rounded corner.
//!
//! The window manager paints each titlebar and handle ([`Frame`]); [`FrameTextures`]
//! copies new pixels into buffers the renderer can draw and turns frames
//! into render elements. A window's buffers go when the window does: each
//! update forgets the windows that are gone.

use std::collections::HashMap;

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::utils::{Logical, Rectangle, Scale};
use vaht_ui::{Color, Corner, CornerArc, Theme, ThemeElement};

use super::{PanelTexture, SceneElement};
use crate::resources::{ResourceKind, ResourceLedger, Tracked};
use crate::window_manager::{Frame, FrameGeometry, WindowId, WindowManager};

/// The frame buffers of every window, by window.
#[derive(Debug, Default)]
pub(crate) struct FrameTextures {
    /// Each window's buffers.
    by_window: HashMap<WindowId, FrameTexture>,
}

impl FrameTextures {
    /// Brings every window's buffers up to date with its frame, colored by
    /// `theme`, and forgets the buffers of windows that are gone.
    pub(crate) fn update(
        &mut self,
        window_manager: &WindowManager,
        theme: &Theme,
        ledger: &ResourceLedger,
    ) {
        self.by_window
            .retain(|window, _| window_manager.window(*window).is_some());
        let metrics = window_manager.metrics();
        for window in window_manager.windows_top_down() {
            self.by_window
                .entry(window.id())
                .or_insert_with(|| FrameTexture::new(ledger))
                .update(window.frame(), &window.frame_geometry(metrics), theme);
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
            _tracked: ledger.track(ResourceKind::FrameTexture),
        }
    }

    /// Copies `frame`'s titlebar and handle pixels if they are newer than
    /// the last copy, and sizes and colors the lines for `geometry`.
    fn update(&mut self, frame: &Frame, geometry: &FrameGeometry, theme: &Theme) {
        self.titlebar.upload(frame.titlebar());
        self.handle.upload(frame.handle());
        let color = theme
            .style(ThemeElement::Border, frame.state())
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
                texture.render_element(renderer, geometry.corner_square(corner), scale)
            })
            .collect();
        for (buffer, rectangle) in self.borders.iter().zip(geometry.borders) {
            if !rectangle.is_empty() {
                elements.push(SceneElement::Solid(SolidColorRenderElement::from_buffer(
                    buffer,
                    rectangle.loc.to_physical_precise_round(scale),
                    scale,
                    1.0,
                    Kind::Unspecified,
                )));
            }
        }
        // Below the lines: the grips' lines are drawn over the handle.
        elements.extend(
            self.titlebar
                .render_element(renderer, geometry.titlebar.loc, scale),
        );
        if !geometry.handle.is_empty() {
            elements.extend(
                self.handle
                    .render_element(renderer, geometry.handle.loc, scale),
            );
        }
        elements
    }

    /// Returns `color` as solid buffers take it.
    fn to_render_color(color: Color) -> [f32; 4] {
        [color.red, color.green, color.blue, color.alpha].map(|channel| f32::from(channel) / 255.0)
    }
}

/// One corner's arc, painted when it changes.
#[derive(Debug)]
struct CornerTexture {
    /// The arc last painted; none before the first.
    arc: Option<CornerArc>,
    /// Its pixels; empty for a square corner.
    texture: PanelTexture,
    /// Counts the arcs painted, so the texture knows new pixels.
    generation: u64,
}

impl CornerTexture {
    /// Creates a corner with no arc yet.
    fn new() -> Self {
        Self {
            arc: None,
            texture: PanelTexture::new(),
            generation: 0,
        }
    }

    /// Paints `arc`, if it is not the one painted already.
    fn update(&mut self, arc: CornerArc) {
        if self.arc == Some(arc) {
            return;
        }
        self.arc = Some(arc);
        if let Some(pixels) = arc.paint() {
            self.generation += 1;
            self.texture.upload_pixels(&pixels, self.generation);
        }
    }

    /// Returns the element that draws the arc in `square`; none for a
    /// square corner.
    fn render_element<R>(
        &self,
        renderer: &mut R,
        square: Rectangle<i32, Logical>,
        scale: Scale<f64>,
    ) -> Option<SceneElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if square.is_empty() {
            return None;
        }
        self.texture.render_element(renderer, square.loc, scale)
    }
}

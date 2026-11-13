//! Every window's frame, as the renderer takes it: the titlebar's and the
//! handle's pixels ([`PanelTexture`]), and one colored buffer per line.
//!
//! The window manager paints each titlebar and handle ([`Frame`]); [`FrameTextures`]
//! copies new pixels into buffers the renderer can draw and turns frames
//! into render elements. A window's buffers go when the window does: each
//! update forgets the windows that are gone.

use std::collections::HashMap;

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::utils::Scale;
use vaht_ui::{Theme, ThemeElement, WidgetState};

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
    /// Counts the buffers while they exist.
    _tracked: Tracked,
}

impl FrameTexture {
    /// Creates empty buffers.
    fn new(ledger: &ResourceLedger) -> Self {
        Self {
            titlebar: PanelTexture::new(),
            handle: PanelTexture::new(),
            borders: std::array::from_fn(|_| SolidColorBuffer::default()),
            _tracked: ledger.track(ResourceKind::FrameTexture),
        }
    }

    /// Copies `frame`'s titlebar and handle pixels if they are newer than
    /// the last copy, and sizes and colors the lines for `geometry`.
    fn update(&mut self, frame: &Frame, geometry: &FrameGeometry, theme: &Theme) {
        self.titlebar.upload(frame.titlebar());
        self.handle.upload(frame.handle());
        let color = Self::border_color(theme, frame.state());
        for (buffer, rectangle) in self.borders.iter_mut().zip(geometry.borders) {
            buffer.update(rectangle.size, color);
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
        let mut elements: Vec<SceneElement<R>> = Vec::new();
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

    /// Returns the border color the theme gives windows in `state`.
    fn border_color(theme: &Theme, state: WidgetState) -> [f32; 4] {
        let color = theme.style(ThemeElement::Border, state).border.color;
        [color.red, color.green, color.blue, color.alpha].map(|channel| f32::from(channel) / 255.0)
    }
}

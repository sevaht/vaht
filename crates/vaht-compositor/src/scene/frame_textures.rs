//! Every window's frame, as the renderer takes it: the titlebar's pixels
//! in a buffer, and one colored buffer per border line.
//!
//! The window manager paints each titlebar ([`Frame`]); [`FrameTextures`]
//! copies new pixels into buffers the renderer can draw and turns frames
//! into render elements. A window's buffers go when the window does: each
//! update forgets the windows that are gone.

use std::collections::HashMap;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::utils::{Buffer as BufferCoordinates, Physical, Point, Rectangle, Scale, Transform};
use vaht_ui::texture::Fill;
use vaht_ui::{Theme, ThemeElement, WidgetState};

use super::SceneElement;
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
    /// The titlebar's pixels as the renderer takes them.
    titlebar: MemoryRenderBuffer,
    /// The titlebar generation last copied into `titlebar`.
    uploaded_generation: Option<u64>,
    /// One colored buffer per border line, in [`FrameGeometry::borders`]'s
    /// order.
    borders: [SolidColorBuffer; 5],
    /// Counts the buffers while they exist.
    _tracked: Tracked,
}

impl FrameTexture {
    /// Creates empty buffers.
    fn new(ledger: &ResourceLedger) -> Self {
        Self {
            titlebar: MemoryRenderBuffer::new(Fourcc::Argb8888, (1, 1), 1, Transform::Normal, None),
            uploaded_generation: None,
            borders: std::array::from_fn(|_| SolidColorBuffer::default()),
            _tracked: ledger.track(ResourceKind::FrameTexture),
        }
    }

    /// Copies `frame`'s titlebar pixels if they are newer than the last
    /// copy, and sizes and colors the borders for `geometry`.
    fn update(&mut self, frame: &Frame, geometry: &FrameGeometry, theme: &Theme) {
        self.upload_titlebar(frame);
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
        let mut elements = Vec::new();
        if self.uploaded_generation.is_some() {
            let location: Point<i32, Physical> =
                geometry.titlebar.loc.to_physical_precise_round(scale);
            match MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location.to_f64(),
                &self.titlebar,
                None,
                None,
                None,
                Kind::Unspecified,
            ) {
                Ok(element) => elements.push(SceneElement::Panel(element)),
                Err(error) => tracing::warn!(?error, "could not draw a titlebar"),
            }
        }
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
        elements
    }

    /// Copies the titlebar's pixels into the renderer's buffer if they are
    /// newer than the last copy.
    fn upload_titlebar(&mut self, frame: &Frame) {
        let titlebar = frame.titlebar();
        let Some(pixels) = titlebar.pixels() else {
            return;
        };
        let generation = titlebar.generation();
        if self.uploaded_generation == Some(generation) {
            return;
        }
        let size: smithay::utils::Size<i32, BufferCoordinates> = (
            i32::try_from(pixels.width()).expect("a titlebar is far narrower than i32::MAX"),
            i32::try_from(pixels.height()).expect("a titlebar is far shorter than i32::MAX"),
        )
            .into();
        let mut context = self.titlebar.render();
        context.resize(size);
        let drawn: Result<(), std::convert::Infallible> = context.draw(|memory| {
            // tiny-skia is premultiplied RGBA; Wayland's ARGB8888 is
            // premultiplied BGRA in memory.
            let (targets, _) = memory.as_chunks_mut::<4>();
            for (target, source) in targets.iter_mut().zip(pixels.pixels()) {
                *target = [source.blue(), source.green(), source.red(), source.alpha()];
            }
            Ok(vec![Rectangle::from_size(size)])
        });
        match drawn {
            Ok(()) => self.uploaded_generation = Some(generation),
            Err(never) => match never {},
        }
    }

    /// Returns the border color the theme gives windows in `state`: the
    /// border's fill, or the start of its gradient.
    fn border_color(theme: &Theme, state: WidgetState) -> [f32; 4] {
        let color = match theme.style(ThemeElement::Border, state).texture.fill {
            Fill::Solid(color) | Fill::Gradient { from: color, .. } => color,
        };
        [color.red, color.green, color.blue, color.alpha].map(|channel| f32::from(channel) / 255.0)
    }
}

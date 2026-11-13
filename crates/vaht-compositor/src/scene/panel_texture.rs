//! One panel's pixels, as the renderer takes them.
//!
//! A [`Panel`] paints into its own pixmap; [`PanelTexture`] copies the
//! pixels into a buffer the renderer draws, only when the panel painted
//! anew, and makes the render element that draws it.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::utils::{
    Buffer as BufferCoordinates, Logical, Physical, Point, Rectangle, Scale, Size, Transform,
};
use vaht_ui::{Panel, PanelContent};

use super::SceneElement;

/// A panel's pixels in a buffer the renderer draws.
#[derive(Debug)]
pub(crate) struct PanelTexture {
    /// The pixels as the renderer takes them.
    buffer: MemoryRenderBuffer,
    /// The panel generation last copied into `buffer`.
    uploaded_generation: Option<u64>,
}

impl PanelTexture {
    /// Creates an empty texture.
    pub(crate) fn new() -> Self {
        Self {
            buffer: MemoryRenderBuffer::new(Fourcc::Argb8888, (1, 1), 1, Transform::Normal, None),
            uploaded_generation: None,
        }
    }

    /// Copies `panel`'s pixels if they are newer than the last copy.
    pub(crate) fn upload<Content: PanelContent>(&mut self, panel: &Panel<Content>) {
        let Some(pixels) = panel.pixels() else {
            return;
        };
        let generation = panel.generation();
        if self.uploaded_generation == Some(generation) {
            return;
        }
        let size: Size<i32, BufferCoordinates> = (
            i32::try_from(pixels.width()).expect("a panel is far narrower than i32::MAX"),
            i32::try_from(pixels.height()).expect("a panel is far shorter than i32::MAX"),
        )
            .into();
        let mut context = self.buffer.render();
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

    /// Returns the element that draws the pixels with their top-left corner
    /// at `location`; none before the first upload.
    pub(crate) fn render_element<R>(
        &self,
        renderer: &mut R,
        location: Point<i32, Logical>,
        scale: Scale<f64>,
    ) -> Option<SceneElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        self.uploaded_generation?;
        let location: Point<i32, Physical> = location.to_physical_precise_round(scale);
        match MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            location.to_f64(),
            &self.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        ) {
            Ok(element) => Some(SceneElement::Panel(element)),
            Err(error) => {
                tracing::warn!(?error, "could not draw a panel");
                None
            }
        }
    }
}

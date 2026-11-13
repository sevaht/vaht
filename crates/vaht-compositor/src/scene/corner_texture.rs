//! One rounded corner's arc, as the renderer takes it.
//!
//! The line around a rounded window frame or menu is straight along its
//! sides and an arc at each rounded corner ([`CornerArc`]). A
//! [`CornerTexture`] paints its corner's arc when it changes, and makes the
//! element that draws it.

use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::utils::{Logical, Rectangle, Scale};
use vaht_ui::CornerArc;

use super::{PanelTexture, SceneElement};

/// One corner's arc, painted when it changes.
#[derive(Debug)]
pub(crate) struct CornerTexture {
    /// The arc last painted; none before the first.
    arc: Option<CornerArc>,
    /// Its pixels; empty for a square corner.
    texture: PanelTexture,
    /// Counts the arcs painted, so the texture knows new pixels.
    generation: u64,
}

impl CornerTexture {
    /// Creates a corner with no arc yet.
    pub(crate) fn new() -> Self {
        Self {
            arc: None,
            texture: PanelTexture::new(),
            generation: 0,
        }
    }

    /// Paints `arc`, if it is not the one painted already.
    pub(crate) fn update(&mut self, arc: CornerArc) {
        if self.arc == Some(arc) {
            return;
        }
        self.arc = Some(arc);
        if let Some(pixels) = arc.paint() {
            self.generation += 1;
            self.texture.upload_pixels(&pixels, self.generation);
        }
    }

    /// Returns the element that draws the arc in `square` at opacity
    /// `alpha`; none for a square corner.
    pub(crate) fn render_element<R>(
        &self,
        renderer: &mut R,
        square: Rectangle<i32, Logical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Option<SceneElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if square.is_empty() {
            return None;
        }
        self.texture
            .render_element(renderer, square.loc, scale, alpha)
    }
}

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
    /// The arc last painted, and the scale it was painted at; none before
    /// the first.
    arc: Option<(CornerArc, f64)>,
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

    /// Paints `arc` with `scale` pixels to a logical one, if it is not the
    /// one painted already; it is drawn the size of its square.
    pub(crate) fn update(&mut self, arc: CornerArc, scale: f64) {
        #[expect(
            clippy::float_cmp,
            reason = "a scale that is what it was, exactly, paints the same"
        )]
        if self
            .arc
            .is_some_and(|(known, known_scale)| known == arc && known_scale == scale)
        {
            return;
        }
        self.arc = Some((arc, scale));
        if let Some(pixels) = arc.paint_scaled(scale) {
            self.generation += 1;
            let side = i32::try_from(arc.radius).unwrap_or(i32::MAX);
            self.texture
                .upload_sized(&pixels, (side, side).into(), self.generation);
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

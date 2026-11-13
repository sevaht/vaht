//! What snapping shows while a window is moved: where it would go, and
//! the layout picker.
//!
//! As fluxland draws them: the preview is a blue rectangle at 30% with a
//! three-pixel outline at 70%, just under the dragged window; the picker
//! is a dark rounded panel of thumbnails, one per layout, its cells grey
//! and the one under the pointer blue. The picker is painted again only
//! when it or the cell under the pointer changes.

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::utils::{Logical, Rectangle, Scale};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect as SkiaRect, Stroke, Transform};
use vaht_ui::CornerRadii;

use super::{Outline, PanelTexture, SceneElement};
use crate::window_manager::{Area, ShownPicker, WindowId, WindowManager};

/// The snap preview's and the layout picker's buffers.
#[derive(Debug, Default)]
pub(crate) struct SnapOverlay {
    /// The window being moved and where it would go, if anywhere.
    preview: Option<(WindowId, Area)>,
    /// The preview's fill.
    fill: SolidColorBuffer,
    /// The preview's outline: top, bottom, left, right.
    lines: [SolidColorBuffer; 4],
    /// The picker's panel, where it is, and what it was painted for;
    /// none while it does not show.
    picker: Option<(Area, PickerKey)>,
    /// The picker's pixels.
    picker_texture: Option<PanelTexture>,
    /// Counts the picker's paintings.
    generation: u64,
}

/// What the picker was painted for: its thumbnails' places, the layouts
/// and the cell under the pointer.
type PickerKey = (Vec<Area>, Vec<Vec<Area>>, Option<(usize, usize)>);

impl SnapOverlay {
    /// The preview's color: a soft blue.
    const BLUE: [f32; 3] = [0.3, 0.5, 0.8];

    /// How wide the preview's outline is.
    const OUTLINE: i32 = 3;

    /// Brings the preview and the picker up to date with the move
    /// `window_manager` follows.
    pub(crate) fn update(&mut self, window_manager: &WindowManager) {
        self.preview = window_manager.snap_preview();
        if let Some((_, area)) = self.preview {
            self.fill.update(area.size, Self::blue(0.3));
            let lines = Outline::lines_around(area, Self::OUTLINE, CornerRadii::SQUARE);
            for (buffer, line) in self.lines.iter_mut().zip(&lines) {
                buffer.update(line.size, Self::blue(0.7));
            }
        }
        let Some(shown) = window_manager.layout_picker() else {
            self.picker = None;
            return;
        };
        let key = Self::key(&shown);
        if self
            .picker
            .as_ref()
            .is_some_and(|(_, painted)| *painted == key)
        {
            return;
        }
        let panel = shown.picker.panel();
        if let Some(pixels) = Self::paint(&shown) {
            self.generation += 1;
            self.picker_texture
                .get_or_insert_with(PanelTexture::new)
                .upload_pixels(&pixels, self.generation);
            self.picker = Some((panel, key));
        }
    }

    /// Returns the elements of the preview for `window`, to go just under
    /// it; none unless it is the window being moved and would snap.
    pub(crate) fn preview_elements<R>(
        &self,
        window: WindowId,
        scale: Scale<f64>,
    ) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some((moved, area)) = self.preview else {
            return Vec::new();
        };
        if moved != window {
            return Vec::new();
        }
        let lines = Outline::lines_around(area, Self::OUTLINE, CornerRadii::SQUARE);
        let solid = |buffer: &SolidColorBuffer, rectangle: Rectangle<i32, Logical>| {
            SceneElement::Solid(SolidColorRenderElement::from_buffer(
                buffer,
                rectangle.loc.to_physical_precise_round(scale),
                scale,
                1.0,
                Kind::Unspecified,
            ))
        };
        self.lines
            .iter()
            .zip(lines)
            .map(|(buffer, line)| solid(buffer, line))
            .chain(std::iter::once(solid(&self.fill, area)))
            .collect()
    }

    /// Returns the element that draws the picker, if it shows.
    pub(crate) fn picker_element<R>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> Option<SceneElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let (panel, _) = self.picker.as_ref()?;
        self.picker_texture
            .as_ref()?
            .render_element(renderer, panel.loc, scale, 1.0)
    }

    /// Returns the preview's blue at opacity `alpha`, premultiplied.
    fn blue(alpha: f32) -> [f32; 4] {
        let [red, green, blue] = Self::BLUE.map(|channel| channel * alpha);
        [red, green, blue, alpha]
    }

    /// Returns what painting `shown` depends on.
    fn key(shown: &ShownPicker<'_>) -> PickerKey {
        let thumbs = shown.picker.thumbs().to_vec();
        let cells = thumbs
            .iter()
            .zip(shown.layouts)
            .map(|(thumb, cells)| cells.iter().map(|cell| cell.area_in(*thumb)).collect())
            .collect();
        (thumbs, cells, shown.hovered)
    }

    /// Paints the picker: its panel, each layout's thumbnail and cells.
    fn paint(shown: &ShownPicker<'_>) -> Option<Pixmap> {
        let panel = shown.picker.panel();
        let mut pixmap = Pixmap::new(to_u32(panel.size.w), to_u32(panel.size.h))?;
        let paint = |red: u8, green: u8, blue: u8, alpha: u8| {
            let mut paint = Paint::default();
            paint.set_color_rgba8(red, green, blue, alpha);
            paint.anti_alias = true;
            paint
        };
        #[expect(clippy::cast_precision_loss, reason = "panel sizes are far below 2^24")]
        let (width, height) = (panel.size.w as f32, panel.size.h as f32);
        let outline = Self::rounded(0.5, 0.5, width - 1.0, height - 1.0, 8.0)?;
        pixmap.fill_path(
            &outline,
            &paint(31, 33, 41, 235),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        pixmap.stroke_path(
            &outline,
            &paint(77, 128, 204, 179),
            &Stroke::default(),
            Transform::identity(),
            None,
        );
        let local = |area: Area, inset: f32| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "panel coordinates are far below 2^24"
            )]
            let (left, top, width, height) = (
                (area.loc.x - panel.loc.x) as f32,
                (area.loc.y - panel.loc.y) as f32,
                area.size.w as f32,
                area.size.h as f32,
            );
            SkiaRect::from_xywh(
                left + inset,
                top + inset,
                width - 2.0 * inset,
                height - 2.0 * inset,
            )
        };
        for (index, (thumb, cells)) in shown.picker.thumbs().iter().zip(shown.layouts).enumerate() {
            if let Some(rect) = local(*thumb, 0.0) {
                pixmap.fill_rect(rect, &paint(51, 56, 69, 255), Transform::identity(), None);
            }
            for (cell_index, cell) in cells.iter().enumerate() {
                let color = if shown.hovered == Some((index, cell_index)) {
                    paint(77, 128, 204, 242)
                } else {
                    paint(128, 133, 148, 204)
                };
                // A gap between neighboring cells.
                if let Some(rect) = local(cell.area_in(*thumb), 1.5) {
                    pixmap.fill_rect(rect, &color, Transform::identity(), None);
                }
            }
        }
        Some(pixmap)
    }

    /// Returns a rectangle with corners of `radius`.
    fn rounded(x: f32, y: f32, width: f32, height: f32, radius: f32) -> Option<tiny_skia::Path> {
        let mut path = PathBuilder::new();
        let (right, bottom) = (x + width, y + height);
        path.move_to(x + radius, y);
        path.line_to(right - radius, y);
        path.quad_to(right, y, right, y + radius);
        path.line_to(right, bottom - radius);
        path.quad_to(right, bottom, right - radius, bottom);
        path.line_to(x + radius, bottom);
        path.quad_to(x, bottom, x, bottom - radius);
        path.line_to(x, y + radius);
        path.quad_to(x, y, x + radius, y);
        path.close();
        path.finish()
    }
}

/// Converts a non-negative extent to a size.
fn to_u32(extent: i32) -> u32 {
    u32::try_from(extent).unwrap_or(0)
}
